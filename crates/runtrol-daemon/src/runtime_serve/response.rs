//! JSON-RPC response construction and wire encoding.

use std::sync::Arc;

use runtrol_ipc::transport::Connection;
use runtrol_provider::CloseMode;
use runtrol_runtime_protocol::{
    ErrorResponse, JsonRpcId, JsonRpcNotification, JsonRpcResponse, ProviderList,
    ProviderUsageList, RuntimeError, RuntimeErrorKind, RuntimeMethod, SuccessResponse,
    WatchEventsResult, WatchProvidersResult, WatchSessionIndexResult, WatchTerminalIndexResult,
};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::runtime_auth::{AuthorizationFailure, AuthorizedIntegration};
use crate::runtime_control::{
    RuntimeAgentGuard, RuntimeControlFailure, RuntimeControlReply, RuntimeCoolGuard,
    RuntimeCooling, RuntimeReturned,
};
use crate::runtime_inventory::RuntimeInventoryFailure;
use crate::runtime_terminal::TerminalView;

use super::connection_state::Watching;

pub(super) struct Answer {
    pub(super) response: JsonRpcResponse,
    pub(super) close: bool,
    pub(super) watching: Option<Watching>,
}

impl Answer {
    pub(super) fn success<T: Serialize>(id: JsonRpcId, result: &T) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: None,
        }
    }

    pub(super) fn success_and_close<T: Serialize>(id: JsonRpcId, result: &T) -> Self {
        Self {
            response: success(id, result),
            close: true,
            watching: None,
        }
    }

    pub(super) fn failure(id: JsonRpcId, failure: AuthorizationFailure) -> Self {
        let close = failure.kind == RuntimeErrorKind::IntegrationRevoked;
        Self {
            response: failure_response(id, failure.kind, failure.message),
            close,
            watching: None,
        }
    }

    pub(super) fn plain(id: JsonRpcId, code: RuntimeErrorKind, message: &str) -> Self {
        Self {
            response: failure_response(id, code, message),
            close: false,
            watching: None,
        }
    }

    pub(super) fn operator_action(
        id: JsonRpcId,
        code: RuntimeErrorKind,
        message: &str,
        action: &str,
        correlation_id: String,
    ) -> Self {
        Self {
            response: JsonRpcResponse::Error(ErrorResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                error: RuntimeError {
                    code,
                    message: message.to_owned(),
                    retryable: true,
                    operator_action: Some(action.to_owned()),
                    correlation_id,
                },
            }),
            close: false,
            watching: None,
        }
    }

    pub(super) fn watching_events(
        id: JsonRpcId,
        result: &WatchEventsResult,
        view: Box<runtrol_core::SessionView>,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::Events {
                subscription_id: result.subscription_id.clone(),
                session_id: result.session_id.clone(),
                view,
            }),
        }
    }

    pub(super) fn watching_index(
        id: JsonRpcId,
        result: &WatchSessionIndexResult,
        authority: AuthorizedIntegration,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::SessionIndex {
                subscription_id: result.subscription_id.clone(),
                last: result.snapshot.clone(),
                authority,
            }),
        }
    }

    pub(super) fn watching_providers(
        id: JsonRpcId,
        result: &WatchProvidersResult,
        updates: watch::Receiver<Arc<ProviderList>>,
        usage: watch::Receiver<Arc<ProviderUsageList>>,
        authority: AuthorizedIntegration,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::Providers {
                subscription_id: result.subscription_id.clone(),
                last: result.snapshot.clone(),
                updates,
                usage,
                authority,
            }),
        }
    }

    pub(super) fn watching_terminal(id: JsonRpcId, view: TerminalView) -> Self {
        let result = view.opened.clone();
        Self {
            response: success(id, &result),
            close: false,
            watching: Some(Watching::Terminal(Box::new(view))),
        }
    }

    pub(super) fn watching_terminal_index(
        id: JsonRpcId,
        result: &WatchTerminalIndexResult,
        updates: watch::Receiver<u64>,
        authority: AuthorizedIntegration,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::TerminalIndex {
                subscription_id: result.subscription_id.clone(),
                last: result.snapshot.clone(),
                updates,
                authority,
            }),
        }
    }
}

pub(super) fn not_ready(id: JsonRpcId) -> Answer {
    Answer::plain(
        id,
        RuntimeErrorKind::NotInitialized,
        "Runtime initialization is not complete",
    )
}

pub(super) fn success<T: Serialize>(id: JsonRpcId, result: &T) -> JsonRpcResponse {
    match serde_json::to_value(result) {
        Ok(result) => JsonRpcResponse::Success(SuccessResponse {
            jsonrpc: "2.0".to_owned(),
            id,
            result,
        }),
        Err(_) => failure_response(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not encode its public result",
        ),
    }
}

pub(super) fn failure_response(
    id: JsonRpcId,
    code: RuntimeErrorKind,
    message: &str,
) -> JsonRpcResponse {
    JsonRpcResponse::Error(ErrorResponse {
        jsonrpc: "2.0".to_owned(),
        id,
        error: RuntimeError::plain(code, message, "runtime-public"),
    })
}

pub(super) fn random_subscription_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    let mut output = String::from("sub_");
    for byte in bytes {
        use core::fmt::Write as _;
        if write!(&mut output, "{byte:02x}").is_err() {
            return Ok(String::new());
        }
    }
    Ok(output)
}

pub(super) async fn send_response(
    connection: &mut Connection,
    response: &JsonRpcResponse,
) -> Result<(), runtrol_ipc::transport::TransportError> {
    let encoded = serde_json::to_vec(response).map_err(|error| {
        runtrol_ipc::transport::TransportError::Io {
            doing: "encoding a public Runtime response",
            detail: error.to_string(),
        }
    })?;
    connection.send(&encoded).await
}

pub(super) async fn send_notification<T: Serialize>(
    connection: &mut Connection,
    method: RuntimeMethod,
    params: &T,
) -> Result<(), runtrol_ipc::transport::TransportError> {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: method.to_string(),
        params: serde_json::to_value(params).map_err(|error| {
            runtrol_ipc::transport::TransportError::Io {
                doing: "encoding public Runtime notification parameters",
                detail: error.to_string(),
            }
        })?,
    };
    let encoded = serde_json::to_vec(&notification).map_err(|error| {
        runtrol_ipc::transport::TransportError::Io {
            doing: "encoding a public Runtime notification",
            detail: error.to_string(),
        }
    })?;
    connection.send(&encoded).await
}

#[cfg(windows)]
pub(super) const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "windows-x86_64"
    } else {
        "windows-aarch64"
    }
}

#[cfg(target_os = "macos")]
pub(super) const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "macos-x86_64"
    } else {
        "macos-aarch64"
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "linux-x86_64"
    } else {
        "linux-aarch64"
    }
}

#[derive(serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyParams {}

#[derive(Serialize)]
pub(super) struct EmptyResult {}

pub(super) fn inventory_failure(id: JsonRpcId, failure: RuntimeInventoryFailure) -> Answer {
    match failure {
        RuntimeInventoryFailure::Unavailable => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "the managed session catalogue is temporarily unavailable",
        ),
        RuntimeInventoryFailure::RootAuthorityChanged => Answer::plain(
            id,
            RuntimeErrorKind::RootDenied,
            "an approved project root no longer names the directory approved locally",
        ),
        RuntimeInventoryFailure::SessionNotFound => Answer::plain(
            id,
            RuntimeErrorKind::SessionNotFound,
            "the Runtime session does not exist in the integration grant",
        ),
    }
}

pub(super) fn runtime_owner_stopped(id: JsonRpcId) -> Answer {
    Answer::plain(
        id,
        RuntimeErrorKind::RuntimeUnavailable,
        "the Runtime session owner stopped",
    )
}

pub(super) fn control_failure(id: JsonRpcId, failure: &RuntimeControlFailure) -> Answer {
    Answer::plain(id, failure.kind, &failure.message)
}

/// Convert a session owner's typed reply into the public JSON-RPC response.
pub(super) async fn runtime_control_answer(
    id: JsonRpcId,
    reply: RuntimeControlReply,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
) -> Answer {
    match reply {
        RuntimeControlReply::Lease(lease) => Answer::success(id, &lease),
        RuntimeControlReply::Done => Answer::success(id, &EmptyResult {}),
        RuntimeControlReply::Approvals(approvals) => Answer::success(id, &approvals),
        RuntimeControlReply::Watching { result, view } => {
            Answer::watching_events(id, &result, view)
        }
        RuntimeControlReply::Failed(failure) => control_failure(id, &failure),
        RuntimeControlReply::Sending {
            mutation,
            taken,
            command,
        } => match perform_runtime_command(mutation, taken, command, returning.clone()).await {
            Some(Ok(())) => Answer::success(id, &EmptyResult {}),
            Some(Err(failure)) => control_failure(id, &failure),
            None => runtime_owner_stopped(id),
        },
        RuntimeControlReply::Cooling(cooling) => {
            match perform_runtime_cool(cooling, returning.clone()).await {
                Some(Ok(())) => Answer::success(id, &EmptyResult {}),
                Some(Err(failure)) => control_failure(id, &failure),
                None => runtime_owner_stopped(id),
            }
        }
        RuntimeControlReply::Opened(result) => Answer::success(id, &result),
        RuntimeControlReply::Opening(_) => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "a session open reservation reached the ordinary control response path",
        ),
    }
}

#[expect(
    clippy::manual_ok_err,
    reason = "Result::ok is forbidden because owner channel loss must remain explicit"
)]
async fn perform_runtime_cool(
    cooling: RuntimeCooling,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) -> Option<Result<(), RuntimeControlFailure>> {
    let RuntimeCooling {
        mutation,
        agent: handed_agent,
        reservation,
    } = cooling;
    let guard = RuntimeCoolGuard::new(mutation, reservation, returning.clone());
    let agent = handed_agent;
    let outcome = agent.close(CloseMode::graceful()).await;
    let reservation = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::Cooled {
            mutation,
            reservation,
            outcome,
            answered,
        })
        .is_err()
    {
        return None;
    }
    match hearing.await {
        Ok(result) => Some(result),
        Err(_) => None,
    }
}

#[expect(
    clippy::manual_ok_err,
    reason = "Result::ok is forbidden because owner channel loss must remain explicit"
)]
async fn perform_runtime_command(
    mutation: runtrol_store::IntegrationMutationKey,
    taken: runtrol_core::TakenAgent,
    command: runtrol_provider::AgentCommand,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) -> Option<Result<(), RuntimeControlFailure>> {
    let runtrol_core::TakenAgent {
        agent: handed_agent,
        lease,
    } = taken;
    let guard = RuntimeAgentGuard::new(mutation, lease, returning.clone());
    let mut agent = handed_agent;
    let outcome = agent.send(command).await;
    let lease = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::Finished {
            mutation,
            taken: runtrol_core::TakenAgent { agent, lease },
            outcome,
            answered,
        })
        .is_err()
    {
        return None;
    }
    match hearing.await {
        Ok(result) => Some(result),
        Err(_) => None,
    }
}

/// Render a local-presence confirmation failure in the caller's operation vocabulary.
pub(super) fn confirmation_failure(
    id: JsonRpcId,
    failure: crate::integration_admin::ConfirmationError,
    what: &str,
) -> Answer {
    use crate::integration_admin::ConfirmationError;
    match failure {
        ConfirmationError::InvalidRequestId => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            &format!("the {what} request identity is invalid or outside its bounded lifetime"),
        ),
        ConfirmationError::IdempotencyConflict => Answer::plain(
            id,
            RuntimeErrorKind::IdempotencyConflict,
            &format!("the {what} request identity was already bound to different parameters"),
        ),
        ConfirmationError::ResourceExhausted => Answer::plain(
            id,
            RuntimeErrorKind::ResourceExhausted,
            &format!("too many {what} requests are awaiting local confirmation"),
        ),
        ConfirmationError::StateUnavailable => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            &format!("Runtime could not verify local {what} confirmation state"),
        ),
    }
}
