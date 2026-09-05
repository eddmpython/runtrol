//! Public Runtime JSON-RPC dispatch and method implementations.

use std::sync::Arc;

use runtrol_runtime_protocol::{
    JsonRpcId, ProviderList, ProviderUsageList, RuntimeErrorKind, RuntimeMethod,
};
use tokio::sync::{mpsc, watch};

use crate::Composed;
use crate::runtime_control::{RuntimeAsked, RuntimeReturned};
use crate::runtime_inventory::RuntimeSessionCatalogue;
use crate::runtime_native_sessions::NativeCursorCodec;

use super::connection_state::PublicState;
use super::integration_requests::{
    grant, initialize, request_integration, rotate_integration_key, watch_integration,
};
use super::provider_requests::{
    focus_native, get_provider_capabilities, list_models, list_native_sessions, native_activity,
    providers_list, providers_usage, providers_watch,
};
use super::response::{Answer, EmptyParams, EmptyResult};
use super::session_control::{open_session, session_operation};
use super::session_requests::{
    forget_session, mutate_native_session, sessions_get, sessions_list, sessions_watch_index,
};
use super::terminal_stream::terminal_operation;

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "public dispatch keeps connection authority, immutable catalogues, owner channels, and the exact JSON-RPC request together"
)]
pub(super) async fn dispatch_public(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Arc<Composed>,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &Arc<NativeCursorCodec>,
    provider_updates: &watch::Sender<Arc<ProviderList>>,
    providers: &ProviderList,
    sessions: &Arc<RuntimeSessionCatalogue>,
    usage: &ProviderUsageList,
    usage_updates: &watch::Receiver<Arc<ProviderUsageList>>,
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if method == RuntimeMethod::PanicStop {
        if serde_json::from_value::<EmptyParams>(params).is_err() {
            Answer::plain(
                id,
                RuntimeErrorKind::InvalidRequest,
                "panic stop parameters are invalid",
            )
        } else {
            match composed.containment.terminate_all() {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(_) => Answer::plain(
                    id,
                    RuntimeErrorKind::Internal,
                    "Runtime could not stop every supervised process",
                ),
            }
        }
    } else {
        match method {
            RuntimeMethod::Initialize => initialize(state, instance_id, composed, id, params).await,
            RuntimeMethod::IntegrationsRequestEnrollment => {
                request_integration(state, composed, id, params)
            }
            RuntimeMethod::IntegrationsWatchEnrollment => {
                watch_integration(state, composed, id, params)
            }
            RuntimeMethod::IntegrationsGetGrant => grant(state, composed, id, params),
            RuntimeMethod::IntegrationsRotateKey => {
                rotate_integration_key(state, composed, id, params).await
            }
            RuntimeMethod::ProvidersList => providers_list(state, composed, providers, id, params),
            RuntimeMethod::ProvidersUsage => {
                providers_usage(state, composed, usage, id, params).await
            }
            RuntimeMethod::ProvidersWatch => {
                providers_watch(state, composed, provider_updates, usage_updates, id, params).await
            }
            RuntimeMethod::ProvidersGetCapabilities => {
                get_provider_capabilities(state, composed, discovering, id, params).await
            }
            RuntimeMethod::ProvidersListModels => {
                list_models(state, composed, discovering, id, params).await
            }
            RuntimeMethod::ProvidersListNativeSessions => {
                list_native_sessions(
                    state,
                    composed,
                    discovering,
                    native_cursors,
                    sessions,
                    id,
                    params,
                )
                .await
            }
            RuntimeMethod::ProvidersNativeActivity => {
                native_activity(state, composed, discovering, id, params).await
            }
            RuntimeMethod::ProvidersFocusNative => focus_native(state, composed, id, params).await,
            RuntimeMethod::SessionsList => sessions_list(state, composed, sessions, id, params),
            RuntimeMethod::SessionsWatchIndex => {
                sessions_watch_index(state, composed, sessions, id, params)
            }
            RuntimeMethod::SessionsGet => sessions_get(state, composed, sessions, id, params),
            RuntimeMethod::SessionsStart
            | RuntimeMethod::SessionsAdoptNative
            | RuntimeMethod::SessionsResume => {
                open_session(
                    state,
                    composed,
                    discovering,
                    native_cursors,
                    sessions,
                    asking,
                    returning,
                    method,
                    id,
                    params,
                )
                .await
            }
            RuntimeMethod::SessionsAcquireControl
            | RuntimeMethod::SessionsRenewControl
            | RuntimeMethod::SessionsReleaseControl
            | RuntimeMethod::SessionsSubmitInput
            | RuntimeMethod::SessionsSubmitBlocks
            | RuntimeMethod::SessionsSetModel
            | RuntimeMethod::SessionsSetMode
            | RuntimeMethod::SessionsWatchEvents
            | RuntimeMethod::SessionsInterrupt
            | RuntimeMethod::SessionsCool
            | RuntimeMethod::ApprovalsListPending
            | RuntimeMethod::ApprovalsRespond => {
                session_operation(
                    state, composed, sessions, asking, returning, method, id, params,
                )
                .await
            }
            RuntimeMethod::SessionsForget => {
                forget_session(state, composed, sessions, asking, returning, id, params).await
            }
            RuntimeMethod::SessionsDeleteNative | RuntimeMethod::SessionsArchiveNative => {
                mutate_native_session(state, composed, discovering, sessions, method, id, params)
                    .await
            }
            RuntimeMethod::TerminalsList
            | RuntimeMethod::TerminalsWatchIndex
            | RuntimeMethod::TerminalsOpen
            | RuntimeMethod::TerminalsAttach
            | RuntimeMethod::TerminalsAcquireControl
            | RuntimeMethod::TerminalsRenewControl
            | RuntimeMethod::TerminalsReleaseControl
            | RuntimeMethod::TerminalsWrite
            | RuntimeMethod::TerminalsResize
            | RuntimeMethod::TerminalsDetach
            | RuntimeMethod::TerminalsStop
            | RuntimeMethod::TerminalsSetDialogue => {
                terminal_operation(
                    state,
                    composed,
                    discovering,
                    native_cursors,
                    providers,
                    sessions,
                    method,
                    id,
                    params,
                )
                .await
            }
            RuntimeMethod::WindowsRegister
            | RuntimeMethod::WindowsUpdate
            | RuntimeMethod::WindowsList
            | RuntimeMethod::WindowsWatchIndex
            | RuntimeMethod::WindowsMirrorOpen
            | RuntimeMethod::WindowsMirrorOutput
            | RuntimeMethod::WindowsMirrorEnd
            | RuntimeMethod::WindowsReveal
            | RuntimeMethod::WindowsWatchReveals => {
                super::window_requests::window_operation(state, composed, method, id, params).await
            }
            RuntimeMethod::Initialized
            | RuntimeMethod::Challenge
            | RuntimeMethod::ProvidersChanged
            | RuntimeMethod::ProvidersWatchEnded
            | RuntimeMethod::ProvidersUsageChanged
            | RuntimeMethod::SessionsEvent
            | RuntimeMethod::SessionsLagged
            | RuntimeMethod::SessionsIndexChanged
            | RuntimeMethod::SessionsIndexEnded
            | RuntimeMethod::TerminalsIndexChanged
            | RuntimeMethod::TerminalsIndexEnded
            | RuntimeMethod::TerminalsOutput
            | RuntimeMethod::TerminalsLagged
            | RuntimeMethod::TerminalsExited
            | RuntimeMethod::WindowsIndexChanged
            | RuntimeMethod::WindowsIndexEnded
            | RuntimeMethod::WindowsRevealRequested
            | RuntimeMethod::WindowsRevealsEnded => Answer::plain(
                id,
                RuntimeErrorKind::InvalidRequest,
                "the method is not a client request in the current state",
            ),
            RuntimeMethod::PanicStop => Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "panic stop dispatch reached an invalid state",
            ),
        }
    }
}

#[cfg(test)]
#[path = "tests/dispatch.rs"]
mod tests;
