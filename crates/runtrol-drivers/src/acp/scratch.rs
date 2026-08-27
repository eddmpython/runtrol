//! One short-lived initialized ACP connection, shared by the questions asked outside any session.
//!
//! Two bounded questions need an agent process that no conversation owns: which conversations it has
//! stored, and where the operator's account with it stands. Both open a process, initialize, ask, and shut
//! down. Keeping that in one place is what stops the second one from growing its own slightly different
//! handshake, and it is the only reason the account reading costs one file rather than three hundred lines.

use bytes::Bytes;
use runtrol_childproc::contain::{ChildGuard, TrackedChild, TrackedCommand};
use runtrol_childproc::{Containment, Program, SpawnError};
use runtrol_provider::{ProviderError, ProviderId};
use serde::Serialize;
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;

use crate::acp::wire;
use crate::framing::jsonrpc;
use crate::framing::{Incoming, LineError, Lines, RequestId};

/// Where to start a child that is being asked about the machine rather than about one folder.
///
/// The system temporary directory, because it exists on every target and is not the operator's
/// home: `runtrol-security`'s workspace rules refuse roots that overlap a credential directory,
/// and starting a provider inside one would put this driver on the wrong side of that boundary for
/// no gain. The agent's answer does not depend on it (measured 2026-08-20 on grok and opencode).
fn spawn_directory() -> std::path::PathBuf {
    std::env::temp_dir()
}

/// One temporary initialized ACP connection, for a bounded question asked outside any session.
pub(super) struct ScratchConnection {
    /// Whose agent this is, carried so a caller's error names the same service.
    pub(super) provider: ProviderId,
    child_guard: ChildGuard,
    child: TrackedChild,
    stdin: Option<ChildStdin>,
    lines: Lines<tokio::process::ChildStdout>,
    next_request: i64,
}

impl ScratchConnection {
    pub(super) async fn start(
        provider: ProviderId,
        program: &Program,
        transport_argv: &[Box<str>],
        directory: Option<&std::path::Path>,
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
            // The agent is asked about folders, not run inside them, and a machine-wide query names
            // none. Measured 2026-08-20: spawning grok and opencode from an unrelated directory
            // changed neither answer. The home directory is used when there is no folder to pick,
            // because a child must start somewhere that exists.
            .current_dir(directory.map_or_else(spawn_directory, std::path::Path::to_path_buf))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let (mut child, child_guard) = command
            .spawn(contained_by)
            .await
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

    pub(super) async fn initialized(&mut self) -> Result<wire::Initialized, ProviderError> {
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
                "initializing the ACP connection",
            )
            .await?;
        let initialized: wire::Initialized =
            serde_json::from_slice(&answer).map_err(|_| ProviderError::Protocol {
                provider: self.provider,
                doing: "initializing the ACP connection",
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

    pub(super) async fn call<P: Serialize>(
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

    pub(super) async fn close(mut self) -> Result<(), ProviderError> {
        drop(self.stdin.take());
        self.child_guard
            .terminate(&mut self.child)
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "stopping the ACP connection",
                source: std::io::Error::other(error),
            })
    }
}

pub(super) fn protocol(provider: ProviderId, detail: impl Into<String>) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing: "listing ACP sessions",
        detail: detail.into(),
    }
}

pub(super) fn spawn_error(
    provider: ProviderId,
    program: &Program,
    error: SpawnError,
) -> ProviderError {
    ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source: std::io::Error::other(error),
    }
}

pub(super) fn line_error(
    provider: ProviderId,
    doing: &'static str,
    error: &LineError,
) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing,
        detail: error.to_string(),
    }
}
