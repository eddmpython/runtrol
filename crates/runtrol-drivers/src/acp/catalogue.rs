//! Bounded ACP v1 `session/list` discovery on a short-lived contained process.

use bytes::Bytes;
use runtrol_childproc::contain::{ChildGuard, TrackedChild, TrackedCommand};
use runtrol_childproc::{Containment, Program, SpawnError};
use runtrol_provider::{
    MAX_NATIVE_ADDITIONAL_DIRECTORIES, MAX_NATIVE_CURSOR_BYTES, MAX_NATIVE_SESSION_ITEMS,
    MAX_NATIVE_TIMESTAMP_BYTES, MAX_NATIVE_TITLE_BYTES, NativeCatalogueCoverage,
    NativeCatalogueSource, NativeResumeCapability, NativeSessionCatalogue, NativeSessionEntry,
    NativeSessionId, NativeSessionQuery, ProviderError, ProviderId,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;

use crate::acp::wire;
use crate::framing::jsonrpc;
use crate::framing::{Incoming, LineError, Lines, RequestId};

const SESSION_LIST: &str = "session/list";

/// Parameters supported by stable ACP v1 session discovery.
#[derive(Serialize)]
struct ListParams<'a> {
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
}

/// One official ACP page, ignoring extension metadata and conversation-derived fields not used by Runtime.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedPage {
    sessions: Vec<ListedSession>,
    next_cursor: Option<Box<str>>,
}

/// The stable ACP fields Runtime may disclose after root authorization.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedSession {
    session_id: Box<str>,
    cwd: Box<str>,
    #[serde(default)]
    additional_directories: Vec<Box<str>>,
    title: Option<Box<str>>,
    updated_at: Option<Box<str>>,
}

/// One temporary initialized ACP connection used only for official catalogue discovery.
struct CatalogueConnection {
    provider: ProviderId,
    child_guard: ChildGuard,
    child: TrackedChild,
    stdin: Option<ChildStdin>,
    lines: Lines<tokio::process::ChildStdout>,
    next_request: i64,
}

impl CatalogueConnection {
    fn start(
        provider: ProviderId,
        program: &Program,
        transport_argv: &[Box<str>],
        query: &NativeSessionQuery,
        contained_by: &Containment,
    ) -> Result<Self, ProviderError> {
        let arguments: Vec<String> = transport_argv.iter().map(ToString::to_string).collect();
        let mut checked = program.leading().to_vec();
        checked.extend_from_slice(&arguments);
        runtrol_childproc::check_all(&checked).map_err(|error| ProviderError::Unsupported {
            provider,
            what: error.to_string(),
            why: "a manifest transport argument cannot be passed on a command line",
        })?;

        let mut command = TrackedCommand::new(program.path().as_std_path());
        command
            .args(program.leading())
            .args(&arguments)
            .current_dir(query.root.as_std_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let (mut child, child_guard) = command
            .spawn(contained_by)
            .map_err(|error| spawn_error(provider, program, error))?;
        let missing = |what: &str| ProviderError::Spawn {
            provider,
            program: program.path().to_string(),
            source: std::io::Error::other(format!("the child has no {what} stream")),
        };
        let stdin = child.stdin.take().ok_or_else(|| missing("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| missing("output"))?;
        Ok(Self {
            provider,
            child_guard,
            child,
            stdin: Some(stdin),
            lines: Lines::new(stdout),
            next_request: 1,
        })
    }

    async fn initialized(&mut self) -> Result<wire::Initialized, ProviderError> {
        let answer = self
            .call(
                wire::INITIALIZE,
                &wire::Initialize {
                    protocol_version: 1,
                    client_capabilities: wire::Empty {},
                    client_info: wire::Implementation {
                        name: "runtrol",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                },
                "initializing ACP session discovery",
            )
            .await?;
        let initialized: wire::Initialized =
            serde_json::from_slice(&answer).map_err(|_| ProviderError::Protocol {
                provider: self.provider,
                doing: "initializing ACP session discovery",
                detail: "the answer has no usable protocolVersion or agentCapabilities".to_owned(),
            })?;
        if initialized.protocol_version != 1 {
            return Err(ProviderError::Unsupported {
                provider: self.provider,
                what: format!("ACP protocol version {}", initialized.protocol_version),
                why: "this build implements stable ACP v1",
            });
        }
        Ok(initialized)
    }

    async fn list(
        &mut self,
        query: &NativeSessionQuery,
        initialized: &wire::Initialized,
    ) -> Result<NativeSessionCatalogue, ProviderError> {
        let Some(session_capabilities) = initialized
            .agent_capabilities
            .get("sessionCapabilities")
            .and_then(serde_json::Value::as_object)
        else {
            return Ok(NativeSessionCatalogue::unsupported(
                "the ACP agent did not advertise sessionCapabilities.list",
            ));
        };
        if !advertised(session_capabilities.get("list")) {
            return Ok(NativeSessionCatalogue::unsupported(
                "the ACP agent did not advertise sessionCapabilities.list",
            ));
        }
        let can_add_directories = advertised(session_capabilities.get("additionalDirectories"));
        let can_resume = initialized
            .agent_capabilities
            .get("loadSession")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let cwd = query.root.to_string();
        let answer = self
            .call(
                SESSION_LIST,
                &ListParams {
                    cwd: &cwd,
                    cursor: query.cursor.as_deref(),
                },
                "listing ACP sessions",
            )
            .await?;
        let page: ListedPage =
            serde_json::from_slice(&answer).map_err(|error| ProviderError::Protocol {
                provider: self.provider,
                doing: "listing ACP sessions",
                detail: error.to_string(),
            })?;
        let limit = usize::from(query.limit).min(MAX_NATIVE_SESSION_ITEMS);
        if page.sessions.len() > limit {
            return Err(protocol(
                self.provider,
                "the ACP page contains too many sessions",
            ));
        }
        if let Some(cursor) = page.next_cursor.as_deref() {
            bounded(
                self.provider,
                "ACP pagination cursor",
                cursor,
                MAX_NATIVE_CURSOR_BYTES,
            )?;
            if query.cursor.as_deref() == Some(cursor) {
                return Err(protocol(
                    self.provider,
                    "the ACP agent repeated the request pagination cursor",
                ));
            }
        }

        let mut sessions = Vec::with_capacity(page.sessions.len());
        for listed in page.sessions {
            if listed.additional_directories.len() > MAX_NATIVE_ADDITIONAL_DIRECTORIES {
                return Err(protocol(
                    self.provider,
                    "an ACP session contains too many additional directories",
                ));
            }
            if !can_add_directories && !listed.additional_directories.is_empty() {
                return Err(protocol(
                    self.provider,
                    "the ACP agent returned additionalDirectories without advertising the capability",
                ));
            }
            if let Some(title) = listed.title.as_deref() {
                bounded(
                    self.provider,
                    "ACP session title",
                    title,
                    MAX_NATIVE_TITLE_BYTES,
                )?;
            }
            if let Some(updated_at) = listed.updated_at.as_deref() {
                bounded(
                    self.provider,
                    "ACP session timestamp",
                    updated_at,
                    MAX_NATIVE_TIMESTAMP_BYTES,
                )?;
            }
            sessions.push(read_session(self.provider, listed, can_resume)?);
        }

        Ok(NativeSessionCatalogue {
            coverage: NativeCatalogueCoverage::Complete {
                source: NativeCatalogueSource::OfficialProtocol,
            },
            sessions,
            next_cursor: page.next_cursor,
        })
    }

    async fn call<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        doing: &'static str,
    ) -> Result<Bytes, ProviderError> {
        let id = RequestId::Number(self.next_request);
        self.next_request = self.next_request.saturating_add(1);
        let line = jsonrpc::write_question(&id, method, params).map_err(|error| {
            ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: error.to_string(),
            }
        })?;
        self.write_line(&line).await?;
        loop {
            let line = self
                .lines
                .next()
                .await
                .map_err(|error| line_error(self.provider, doing, &error))?
                .ok_or_else(|| ProviderError::Protocol {
                    provider: self.provider,
                    doing,
                    detail: "the provider stream ended before answering".to_owned(),
                })?;
            match jsonrpc::read(&line).map_err(|error| ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: error.to_string(),
            })? {
                Incoming::Answer {
                    id: answer_id,
                    outcome,
                } if answer_id == id => {
                    return outcome.map_err(|error| ProviderError::NativeRefused {
                        provider: self.provider,
                        doing,
                        detail: format!("{} ({})", error.message, error.code),
                    });
                }
                Incoming::Question { id, .. } => {
                    let refusal = jsonrpc::write_error(
                        &id,
                        -32601,
                        "runtrol does not serve this client method during discovery",
                    );
                    self.write_line(&refusal).await?;
                }
                Incoming::Answer { .. } | Incoming::Report { .. } => {}
            }
        }
    }

    async fn write_line(&mut self, text: &str) -> Result<(), ProviderError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| ProviderError::Protocol {
            provider: self.provider,
            doing: "sending an ACP discovery frame",
            detail: "the discovery input has already been closed".to_owned(),
        })?;
        stdin
            .write_all(format!("{text}\n").as_bytes())
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "writing an ACP discovery frame",
                source: error,
            })?;
        stdin.flush().await.map_err(|error| ProviderError::Io {
            provider: self.provider,
            doing: "flushing an ACP discovery frame",
            source: error,
        })
    }

    async fn close(mut self) -> Result<(), ProviderError> {
        drop(self.stdin.take());
        self.child_guard
            .terminate(&mut self.child)
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "stopping ACP session discovery",
                source: std::io::Error::other(error),
            })
    }
}

pub(super) async fn list(
    provider: ProviderId,
    program: &Program,
    transport_argv: &[Box<str>],
    query: NativeSessionQuery,
    contained_by: &Containment,
) -> Result<NativeSessionCatalogue, ProviderError> {
    if query.limit == 0 || usize::from(query.limit) > MAX_NATIVE_SESSION_ITEMS {
        return Err(protocol(
            provider,
            "the requested ACP page limit is invalid",
        ));
    }
    if let Some(cursor) = query.cursor.as_deref() {
        bounded(
            provider,
            "ACP pagination cursor",
            cursor,
            MAX_NATIVE_CURSOR_BYTES,
        )?;
    }
    let mut connection =
        CatalogueConnection::start(provider, program, transport_argv, &query, contained_by)?;
    let outcome = async {
        let initialized = connection.initialized().await?;
        connection.list(&query, &initialized).await
    }
    .await;
    let cleanup = connection.close().await;
    match (outcome, cleanup) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(catalogue), Ok(())) => Ok(catalogue),
    }
}

fn advertised(value: Option<&serde_json::Value>) -> bool {
    !matches!(
        value,
        None | Some(serde_json::Value::Null | serde_json::Value::Bool(false))
    )
}

fn read_session(
    provider: ProviderId,
    listed: ListedSession,
    can_resume: bool,
) -> Result<NativeSessionEntry, ProviderError> {
    let native =
        NativeSessionId::new(&listed.session_id).map_err(|error| ProviderError::Protocol {
            provider,
            doing: "listing ACP sessions",
            detail: format!("an ACP session identifier is unusable: {error}"),
        })?;
    Ok(NativeSessionEntry {
        native,
        cwd: listed.cwd,
        additional_directories: listed.additional_directories,
        title: listed.title,
        updated_at: listed.updated_at,
        resume: if can_resume {
            NativeResumeCapability::Available
        } else {
            NativeResumeCapability::Unavailable
        },
    })
}

fn bounded(
    provider: ProviderId,
    what: &'static str,
    value: &str,
    limit: usize,
) -> Result<(), ProviderError> {
    if value.len() <= limit && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(protocol(
            provider,
            format!("the provider returned an oversized or invalid {what}"),
        ))
    }
}

fn protocol(provider: ProviderId, detail: impl Into<String>) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing: "listing ACP sessions",
        detail: detail.into(),
    }
}

fn spawn_error(provider: ProviderId, program: &Program, error: SpawnError) -> ProviderError {
    ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source: std::io::Error::other(error),
    }
}

fn line_error(provider: ProviderId, doing: &'static str, error: &LineError) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_capability_objects_are_advertised_by_acp() {
        assert!(advertised(Some(&serde_json::json!({}))));
        assert!(!advertised(None));
        assert!(!advertised(Some(&serde_json::Value::Null)));
        assert!(!advertised(Some(&serde_json::Value::Bool(false))));
    }

    #[test]
    fn the_list_decoder_drops_extension_metadata() {
        let page: ListedPage = serde_json::from_str(
            r#"{"sessions":[{"sessionId":"native-1","cwd":"/work","additionalDirectories":[],"title":"Provider title","updatedAt":"2026-08-13T00:00:00Z","_meta":{"preview":"must not cross"}}]}"#,
        )
        .expect("stable fields decode");
        assert_eq!(page.sessions.len(), 1);
        assert!(page.next_cursor.is_none());
    }
}
