//! Bounded ACP v1 `session/list` discovery on a short-lived contained process.

use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    MAX_NATIVE_ADDITIONAL_DIRECTORIES, MAX_NATIVE_CURSOR_BYTES, MAX_NATIVE_SESSION_ITEMS,
    MAX_NATIVE_TIMESTAMP_BYTES, MAX_NATIVE_TITLE_BYTES, NativeCatalogueCoverage,
    NativeCatalogueSource, NativeResumeCapability, NativeSessionCatalogue, NativeSessionEntry,
    NativeSessionId, NativeSessionQuery, ProviderError, ProviderId,
};
use serde::{Deserialize, Serialize};

use crate::acp::scratch::{ScratchConnection, protocol};
use crate::acp::wire;

const SESSION_LIST: &str = "session/list";

/// Parameters supported by stable ACP v1 session discovery.
///
/// `cwd` is a filter in this protocol, not a scope: measured 2026-08-20 against grok 1.0.5 and
/// opencode 1.2.27, omitting it returns every session the agent knows (30 and 36 rows across as
/// many distinct folders), and supplying an unrelated folder returns none. So the field is omitted
/// for a machine-wide query rather than filled with a guess.
#[derive(Serialize)]
struct ListParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<&'a str>,
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
    let mut connection = ScratchConnection::start(
        provider,
        program,
        transport_argv,
        query
            .root
            .as_ref()
            .map(runtrol_provider::AbsPath::as_std_path),
        contained_by,
    )
    .await?;
    let outcome = async {
        let initialized = connection.initialized().await?;
        list_pages(&mut connection, &query, &initialized).await
    }
    .await;
    let cleanup = connection.close().await;
    match (outcome, cleanup) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(catalogue), Ok(())) => Ok(catalogue),
    }
}

/// One page of this agent's stored conversations, over an already initialized connection.
async fn list_pages(
    connection: &mut ScratchConnection,
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
    let cwd = query.root.as_ref().map(ToString::to_string);
    let answer = connection
        .call(
            SESSION_LIST,
            &ListParams {
                cwd: cwd.as_deref(),
                cursor: query.cursor.as_deref(),
            },
            "listing ACP sessions",
        )
        .await?;
    let page: ListedPage =
        serde_json::from_slice(&answer).map_err(|error| ProviderError::Protocol {
            provider: connection.provider,
            doing: "listing ACP sessions",
            detail: error.to_string(),
        })?;
    let limit = usize::from(query.limit).min(MAX_NATIVE_SESSION_ITEMS);
    if page.sessions.len() > limit {
        return Err(protocol(
            connection.provider,
            "the ACP page contains too many sessions",
        ));
    }
    if let Some(cursor) = page.next_cursor.as_deref() {
        bounded(
            connection.provider,
            "ACP pagination cursor",
            cursor,
            MAX_NATIVE_CURSOR_BYTES,
        )?;
        if query.cursor.as_deref() == Some(cursor) {
            return Err(protocol(
                connection.provider,
                "the ACP agent repeated the request pagination cursor",
            ));
        }
    }

    let mut sessions = Vec::with_capacity(page.sessions.len());
    for listed in page.sessions {
        if listed.additional_directories.len() > MAX_NATIVE_ADDITIONAL_DIRECTORIES {
            return Err(protocol(
                connection.provider,
                "an ACP session contains too many additional directories",
            ));
        }
        if !can_add_directories && !listed.additional_directories.is_empty() {
            return Err(protocol(
                connection.provider,
                "the ACP agent returned additionalDirectories without advertising the capability",
            ));
        }
        if let Some(title) = listed.title.as_deref() {
            bounded(
                connection.provider,
                "ACP session title",
                title,
                MAX_NATIVE_TITLE_BYTES,
            )?;
        }
        if let Some(updated_at) = listed.updated_at.as_deref() {
            bounded(
                connection.provider,
                "ACP session timestamp",
                updated_at,
                MAX_NATIVE_TIMESTAMP_BYTES,
            )?;
        }
        sessions.push(read_session(connection.provider, listed, can_resume)?);
    }

    Ok(NativeSessionCatalogue {
        coverage: NativeCatalogueCoverage::Complete {
            source: NativeCatalogueSource::OfficialProtocol,
        },
        sessions,
        next_cursor: page.next_cursor,
    })
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
