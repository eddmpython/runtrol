//! Connection-bound terminal dispatch, streaming, and root revalidation.

use std::sync::Arc;
use std::time::Duration;

use base64ct::Encoding as _;
use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    AppScope, JsonRpcId, JsonRpcRequest, JsonRpcResponse, ListTerminalsParams, ProviderList,
    RuntimeErrorKind, RuntimeMethod, TerminalAcquireControlParams, TerminalAttachParams,
    TerminalControlParams, TerminalDetachParams, TerminalExitedNotification,
    TerminalIndexChangedNotification, TerminalIndexEndReason, TerminalIndexEndedNotification,
    TerminalLaggedNotification, TerminalOpenParams, TerminalOutputNotification,
    TerminalResizeParams, TerminalStopParams, TerminalWriteParams, WatchTerminalIndexParams,
    WatchTerminalIndexResult,
};
use tokio::sync::watch;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_inventory::RuntimeSessionCatalogue;
use crate::runtime_native_sessions::NativeCursorCodec;
use crate::runtime_terminal::{TerminalRuntimeFailure, TerminalView, has_scopes, run_root_check};

#[cfg(not(windows))]
use super::authority::current_authority_row;
use super::authority::{authorized_scopes, refresh_current, refresh_current_in_place};
use super::connection_state::{PublicState, RelayOutcome};
use super::response::{
    Answer, EmptyResult, failure_response, random_subscription_id, send_notification,
    send_response, success,
};

struct TerminalViewResponse {
    response: JsonRpcResponse,
    detach: bool,
}

impl TerminalViewResponse {
    const fn continuing(response: JsonRpcResponse) -> Self {
        Self {
            response,
            detach: false,
        }
    }

    const fn detaching(response: JsonRpcResponse) -> Self {
        Self {
            response,
            detach: true,
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the closed terminal method table keeps each DTO, composite scope set, and adapter call adjacent"
)]
pub(super) async fn terminal_operation(
    state: &mut PublicState,
    composed: &Arc<Composed>,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &NativeCursorCodec,
    providers: &ProviderList,
    sessions: &RuntimeSessionCatalogue,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    match method {
        RuntimeMethod::TerminalsList => {
            if serde_json::from_value::<ListTerminalsParams>(params).is_err() {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal list parameters are invalid",
                );
            }
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionList]) {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed.runtime_terminals.list(composed, &authority).await {
                Ok(snapshot) => Answer::success(id, &snapshot),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsWatchIndex => {
            if serde_json::from_value::<WatchTerminalIndexParams>(params).is_err() {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal index watch parameters are invalid",
                );
            }
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionList]) {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            let updates = composed.runtime_terminals.changes(composed).await;
            let snapshot = match composed.runtime_terminals.list(composed, &authority).await {
                Ok(snapshot) => snapshot,
                Err(failure) => return terminal_failure(id, failure),
            };
            let Ok(subscription_id) = random_subscription_id() else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::Internal,
                    "Runtime could not allocate a terminal index subscription",
                );
            };
            let result = WatchTerminalIndexResult {
                subscription_id,
                snapshot,
            };
            Answer::watching_terminal_index(id, &result, updates, authority)
        }
        RuntimeMethod::TerminalsOpen => {
            let Ok(params) = serde_json::from_value::<TerminalOpenParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal open parameters are invalid",
                );
            };
            let lifecycle_scope = match &params.target {
                runtrol_runtime_protocol::TerminalOpenTarget::Fresh => AppScope::SessionStart,
                runtrol_runtime_protocol::TerminalOpenTarget::Native { .. } => {
                    AppScope::SessionResume
                }
            };
            let authority = match authorized_scopes(
                state,
                composed,
                &[lifecycle_scope, AppScope::SessionOutputRead],
            ) {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .open(
                    composed,
                    discovering,
                    native_cursors,
                    providers,
                    sessions,
                    authority,
                    &params,
                )
                .await
            {
                Ok(view) => Answer::watching_terminal(id, view),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsAttach => {
            let Ok(params) = serde_json::from_value::<TerminalAttachParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal attach parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionOutputRead])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .attach(composed, authority, &params)
                .await
            {
                Ok(view) => Answer::watching_terminal(id, view),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsAcquireControl => {
            let Ok(params) = serde_json::from_value::<TerminalAcquireControlParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control acquisition parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionInputWrite])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .acquire(composed, &authority, &params)
                .await
            {
                Ok(lease) => Answer::success(id, &lease),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsRenewControl => {
            let Ok(params) = serde_json::from_value::<TerminalControlParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control renewal parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionInputWrite])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .renew(composed, &authority, &params)
                .await
            {
                Ok(lease) => Answer::success(id, &lease),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsReleaseControl => {
            let Ok(params) = serde_json::from_value::<TerminalControlParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control release parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionInputWrite])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .release(composed, &authority, &params)
                .await
            {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsWrite => {
            let Ok(params) = serde_json::from_value::<TerminalWriteParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal write parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionInputWrite])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .write(composed, &authority, &params)
                .await
            {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsResize => {
            let Ok(params) = serde_json::from_value::<TerminalResizeParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal resize parameters are invalid",
                );
            };
            let authority = match authorized_scopes(state, composed, &[AppScope::SessionInputWrite])
            {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .resize(composed, &authority, &params)
                .await
            {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsStop => {
            let Ok(params) = serde_json::from_value::<TerminalStopParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal stop parameters are invalid",
                );
            };
            let authority = match authorized_scopes(
                state,
                composed,
                &[AppScope::SessionStop, AppScope::SessionInputWrite],
            ) {
                Ok(authority) => authority.clone(),
                Err(failure) => return Answer::failure(id, failure),
            };
            match composed
                .runtime_terminals
                .stop(composed, &authority, &params)
                .await
            {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(failure) => terminal_failure(id, failure),
            }
        }
        RuntimeMethod::TerminalsDetach => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "terminal detach belongs to the connection-bound terminal view",
        ),
        _ => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "non-terminal method reached terminal dispatch",
        ),
    }
}

fn terminal_failure(id: JsonRpcId, failure: TerminalRuntimeFailure) -> Answer {
    Answer::plain(id, failure.kind, failure.message)
}

#[expect(
    clippy::too_many_lines,
    reason = "one dedicated terminal transport orders exact output, lag replacement, exit, authority notifications, and control replies"
)]
pub(super) async fn relay_terminal(
    connection: &mut Connection,
    composed: &Composed,
    mut view: TerminalView,
) -> RelayOutcome {
    let mut sequence = 1_u64;
    let mut primary_authority_updates = composed.integration_authority.subscribe();
    let mut draining_authority_updates = composed.generation_authority.subscribe();
    // Subscribe first, then read. A mutation between terminal admission and this relay is either present in
    // the snapshot below or wakes one of these receivers; there is no gap where passive output can retain it.
    if refresh_terminal_authority(composed, &mut view).is_err() {
        return RelayOutcome::CloseConnection;
    }
    let mut root_check: Option<tokio::task::JoinHandle<TerminalRootCheck>> = None;
    let first_check = tokio::time::Instant::now() + Duration::from_millis(500);
    let mut root_tick = tokio::time::interval_at(first_check, Duration::from_millis(500));
    root_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Only a draining generation needs a clock: it must fail closed when successor relay updates stop.
    // Primary generations wake on the commit-coupled notification above and perform no periodic store read.
    let mut authority_tick = tokio::time::interval_at(first_check, Duration::from_millis(500));
    authority_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let already_exited = *view.attachment.exited.borrow();
    if let Some(exit_code) = already_exited {
        drop(
            send_notification(
                connection,
                RuntimeMethod::TerminalsExited,
                &TerminalExitedNotification {
                    view_id: view.opened.view_id,
                    exit_code,
                },
            )
            .await,
        );
        return RelayOutcome::CloseConnection;
    }
    loop {
        tokio::select! {
            biased;
            changed = primary_authority_updates.changed() => {
                if changed.is_err() || refresh_terminal_authority(composed, &mut view).is_err() {
                    return RelayOutcome::CloseConnection;
                }
            }
            changed = draining_authority_updates.changed() => {
                if changed.is_err() || refresh_terminal_authority(composed, &mut view).is_err() {
                    return RelayOutcome::CloseConnection;
                }
            }
            checked = async {
                match root_check.as_mut() {
                    Some(check) => check.await,
                    None => std::future::pending().await,
                }
            }, if root_check.is_some() => {
                root_check = None;
                let Ok(checked) = checked else {
                    return RelayOutcome::CloseConnection;
                };
                let current_stamp = (
                    view.authority.grant.key_generation,
                    view.authority.grant.grant_generation,
                    view.hosted.generation,
                );
                // A failed or timed-out filesystem proof is never made safe by an authority update. Only a
                // successful proof may be stale, in which case it authorizes nothing and the next tick retries.
                if !checked.allowed {
                    return RelayOutcome::CloseConnection;
                }
                if checked.stamp == current_stamp {
                    view.remember_root_proof();
                } else {
                    root_tick.reset_immediately();
                }
            }
            _ = authority_tick.tick(), if composed.draining.load(std::sync::atomic::Ordering::Acquire) => {
                if refresh_terminal_authority(composed, &mut view).is_err() {
                    return RelayOutcome::CloseConnection;
                }
            }
            _ = root_tick.tick(), if root_check.is_none() => {
                #[cfg(windows)]
                {
                    root_check = Some(tokio::spawn(check_pinned_terminal_root(
                        composed.terminal_root_checks.clone(),
                        view.pinned_root_guard(),
                        (
                            view.authority.grant.key_generation,
                            view.authority.grant.grant_generation,
                            view.hosted.generation,
                        ),
                    )));
                }
                #[cfg(not(windows))]
                {
                    let Ok(row) = current_authority_row(composed, &view.authority) else {
                        return RelayOutcome::CloseConnection;
                    };
                    root_check = Some(tokio::spawn(check_terminal_roots(
                        composed.terminal_root_checks.clone(),
                        row,
                        view.hosted.workspace.clone(),
                        view.hosted.generation,
                    )));
                }
            }
            changed = view.attachment.exited.changed() => {
                if changed.is_err() {
                    return RelayOutcome::CloseConnection;
                }
                let Some(exit_code) = *view.attachment.exited.borrow() else {
                    continue;
                };
                while let Ok(chunk) = view.attachment.live.try_recv() {
                    if chunk.len() > runtrol_runtime_protocol::MAX_TERMINAL_OUTPUT_BYTES {
                        return RelayOutcome::CloseConnection;
                    }
                    let notification = TerminalOutputNotification {
                        view_id: view.opened.view_id.clone(),
                        sequence,
                        bytes_base64: base64ct::Base64::encode_string(&chunk),
                    };
                    sequence = sequence.saturating_add(1);
                    if send_notification(connection, RuntimeMethod::TerminalsOutput, &notification)
                        .await
                        .is_err()
                    {
                        return RelayOutcome::CloseConnection;
                    }
                }
                drop(
                    send_notification(
                        connection,
                        RuntimeMethod::TerminalsExited,
                        &TerminalExitedNotification {
                            view_id: view.opened.view_id,
                            exit_code,
                        },
                    )
                    .await,
                );
                return RelayOutcome::CloseConnection;
            }
            inbound = connection.recv() => {
                let Ok(Some(payload)) = inbound else {
                    return RelayOutcome::CloseConnection;
                };
                let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&payload) else {
                    return RelayOutcome::CloseConnection;
                };
                let handled = terminal_view_request(composed, &mut view, request).await;
                if send_response(connection, &handled.response).await.is_err() {
                    return RelayOutcome::CloseConnection;
                }
                if handled.detach {
                    return RelayOutcome::ResumeRequests;
                }
            }
            output = view.attachment.live.recv() => {
                match output {
                    Ok(chunk) => {
                        if chunk.len() > runtrol_runtime_protocol::MAX_TERMINAL_OUTPUT_BYTES {
                            return RelayOutcome::CloseConnection;
                        }
                        let notification = TerminalOutputNotification {
                            view_id: view.opened.view_id.clone(),
                            sequence,
                            bytes_base64: base64ct::Base64::encode_string(&chunk),
                        };
                        sequence = sequence.saturating_add(1);
                        if send_notification(connection, RuntimeMethod::TerminalsOutput, &notification)
                            .await
                            .is_err()
                        {
                            return RelayOutcome::CloseConnection;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                        let fresh = view
                            .hosted
                            .terminal
                            .attach()
                            .await;
                        if fresh.snapshot.len() > runtrol_runtime_protocol::MAX_TERMINAL_SCREEN_BYTES {
                            return RelayOutcome::CloseConnection;
                        }
                        view.attachment.live = fresh.live;
                        view.attachment.exited = fresh.exited;
                        let notification = TerminalLaggedNotification {
                            view_id: view.opened.view_id.clone(),
                            lost_chunks: lost,
                            screen_base64: base64ct::Base64::encode_string(&fresh.snapshot),
                            next_sequence: sequence,
                        };
                        if send_notification(connection, RuntimeMethod::TerminalsLagged, &notification)
                            .await
                            .is_err()
                        {
                            return RelayOutcome::CloseConnection;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return RelayOutcome::CloseConnection;
                    }
                }
            }
        }
    }
}

struct TerminalRootCheck {
    stamp: (u64, u64, u64),
    allowed: bool,
}

/// Validate the pinned Windows directory handle away from terminal input and output tasks.
#[cfg(windows)]
async fn check_pinned_terminal_root(
    permits: Arc<tokio::sync::Semaphore>,
    guard: Arc<tokio::sync::Mutex<runtrol_security::ProjectRootGuard>>,
    stamp: (u64, u64, u64),
) -> TerminalRootCheck {
    let checked = run_root_check(permits, move || guard.blocking_lock().validate().is_ok()).await;
    TerminalRootCheck {
        stamp,
        allowed: matches!(checked, Ok(true)),
    }
}

/// Check a quiet output view on the blocking pool, with one daemon-wide bound and a fail-closed deadline.
#[cfg(not(windows))]
async fn check_terminal_roots(
    permits: Arc<tokio::sync::Semaphore>,
    row: Arc<runtrol_store::IntegrationRow>,
    workspace: runtrol_provider::AbsPath,
    terminal_generation: u64,
) -> TerminalRootCheck {
    let stamp = (
        row.key_generation,
        row.grant_generation,
        terminal_generation,
    );
    let checked = run_root_check(permits, move || {
        crate::runtime_terminal::validate_workspace_roots(&row, &workspace).is_ok()
    })
    .await;
    TerminalRootCheck {
        stamp,
        allowed: matches!(checked, Ok(true)),
    }
}

fn refresh_terminal_authority(composed: &Composed, view: &mut TerminalView) -> Result<(), ()> {
    refresh_current_in_place(composed, &mut view.authority).map_err(drop)?;
    view.refresh_root_authority().map_err(drop)?;
    if has_scopes(&view.authority.grant, &[AppScope::SessionOutputRead]) {
        Ok(())
    } else {
        Err(())
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the dedicated stream has one closed terminal-control method table with per-method DTO validation"
)]
async fn terminal_view_request(
    composed: &Composed,
    view: &mut TerminalView,
    request: JsonRpcRequest,
) -> TerminalViewResponse {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return TerminalViewResponse::continuing(failure_response(
            id,
            RuntimeErrorKind::InvalidRequest,
            "JSON-RPC version must be 2.0",
        ));
    }
    let Ok(method) = request.method.parse::<RuntimeMethod>() else {
        return TerminalViewResponse::continuing(failure_response(
            id,
            RuntimeErrorKind::MethodNotFound,
            "the public Runtime method does not exist",
        ));
    };
    let scopes: &[AppScope] = match method {
        RuntimeMethod::TerminalsDetach => &[AppScope::SessionOutputRead],
        RuntimeMethod::TerminalsAcquireControl
        | RuntimeMethod::TerminalsRenewControl
        | RuntimeMethod::TerminalsReleaseControl
        | RuntimeMethod::TerminalsWrite
        | RuntimeMethod::TerminalsResize => &[AppScope::SessionInputWrite],
        RuntimeMethod::TerminalsStop => &[AppScope::SessionStop, AppScope::SessionInputWrite],
        _ => {
            return TerminalViewResponse::continuing(failure_response(
                id,
                RuntimeErrorKind::InvalidRequest,
                "the dedicated terminal view accepts only terminal control requests",
            ));
        }
    };
    if let Err(failure) = refresh_current_in_place(composed, &mut view.authority) {
        return TerminalViewResponse::continuing(failure_response(
            id,
            failure.kind,
            failure.message,
        ));
    }
    if let Err(failure) = view.refresh_root_authority() {
        return TerminalViewResponse::continuing(failure_response(
            id,
            failure.kind,
            failure.message,
        ));
    }
    if !has_scopes(&view.authority.grant, scopes) {
        return TerminalViewResponse::continuing(failure_response(
            id,
            RuntimeErrorKind::ScopeDenied,
            "the integration grant lacks a required terminal scope",
        ));
    }
    let response = match method {
        RuntimeMethod::TerminalsDetach => {
            let Ok(params) = serde_json::from_value::<TerminalDetachParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal detach parameters are invalid",
                ));
            };
            if params.view_id != view.opened.view_id
                || params.terminal_id != view.opened.terminal.terminal_id
            {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::TerminalNotFound,
                    "the terminal view is not bound to this connection",
                ));
            }
            return TerminalViewResponse::detaching(success(id, &EmptyResult {}));
        }
        RuntimeMethod::TerminalsAcquireControl => {
            let Ok(params) = serde_json::from_value::<TerminalAcquireControlParams>(request.params)
            else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control acquisition parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .acquire(composed, &view.authority, &params)
                .await
            {
                Ok(lease) => success(id, &lease),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        RuntimeMethod::TerminalsRenewControl => {
            let Ok(params) = serde_json::from_value::<TerminalControlParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control renewal parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .renew(composed, &view.authority, &params)
                .await
            {
                Ok(lease) => success(id, &lease),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        RuntimeMethod::TerminalsReleaseControl => {
            let Ok(params) = serde_json::from_value::<TerminalControlParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal control release parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .release(composed, &view.authority, &params)
                .await
            {
                Ok(()) => success(id, &EmptyResult {}),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        RuntimeMethod::TerminalsWrite => {
            let Ok(params) = serde_json::from_value::<TerminalWriteParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal write parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .write_view(composed, view, &params)
                .await
            {
                Ok(()) => success(id, &EmptyResult {}),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        RuntimeMethod::TerminalsResize => {
            let Ok(params) = serde_json::from_value::<TerminalResizeParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal resize parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .resize(composed, &view.authority, &params)
                .await
            {
                Ok(()) => success(id, &EmptyResult {}),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        RuntimeMethod::TerminalsStop => {
            let Ok(params) = serde_json::from_value::<TerminalStopParams>(request.params) else {
                return TerminalViewResponse::continuing(failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "terminal stop parameters are invalid",
                ));
            };
            match composed
                .runtime_terminals
                .stop(composed, &view.authority, &params)
                .await
            {
                Ok(()) => success(id, &EmptyResult {}),
                Err(failure) => failure_response(id, failure.kind, failure.message),
            }
        }
        _ => failure_response(
            id,
            RuntimeErrorKind::Internal,
            "non-terminal method reached terminal view dispatch",
        ),
    };
    TerminalViewResponse::continuing(response)
}

pub(super) async fn relay_terminal_index(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: String,
    mut last: runtrol_runtime_protocol::TerminalIndexSnapshot,
    mut updates: watch::Receiver<u64>,
    mut authority: AuthorizedIntegration,
) {
    enum Wake {
        Structure,
        Authority,
    }

    let mut authority_tick = tokio::time::interval(Duration::from_millis(500));
    authority_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let wake = tokio::select! {
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            changed = updates.changed() => {
                if changed.is_err() {
                    send_terminal_index_end(
                        connection,
                        subscription_id,
                        TerminalIndexEndReason::RuntimeUnavailable,
                    ).await;
                    return;
                }
                Wake::Structure
            }
            _ = authority_tick.tick() => Wake::Authority,
        };
        let previous_grant_generation = authority.grant.grant_generation;
        authority = match refresh_current(composed, &authority) {
            Ok(current) if has_scopes(&current.grant, &[AppScope::SessionList]) => current,
            Ok(_) => {
                send_terminal_index_end(
                    connection,
                    subscription_id,
                    TerminalIndexEndReason::AuthorityChanged,
                )
                .await;
                return;
            }
            Err(failure) => {
                let reason = if failure.kind == RuntimeErrorKind::IntegrationRevoked {
                    TerminalIndexEndReason::IntegrationRevoked
                } else {
                    TerminalIndexEndReason::AuthorityChanged
                };
                send_terminal_index_end(connection, subscription_id, reason).await;
                return;
            }
        };
        let Ok(roots) = composed
            .runtime_terminals
            .validated_roots(composed, &authority)
            .await
        else {
            send_terminal_index_end(
                connection,
                subscription_id,
                TerminalIndexEndReason::AuthorityChanged,
            )
            .await;
            return;
        };
        if matches!(wake, Wake::Authority)
            && authority.grant.grant_generation == previous_grant_generation
        {
            composed.runtime_terminals.refresh_memory(composed).await;
            continue;
        }
        let Ok(snapshot) = composed
            .runtime_terminals
            .list_validated(composed, &roots)
            .await
        else {
            send_terminal_index_end(
                connection,
                subscription_id,
                TerminalIndexEndReason::AuthorityChanged,
            )
            .await;
            return;
        };
        if snapshot == last {
            continue;
        }
        last = snapshot.clone();
        let notification = TerminalIndexChangedNotification {
            subscription_id: subscription_id.clone(),
            snapshot,
        };
        if send_notification(
            connection,
            RuntimeMethod::TerminalsIndexChanged,
            &notification,
        )
        .await
        .is_err()
        {
            return;
        }
    }
}

async fn send_terminal_index_end(
    connection: &mut Connection,
    subscription_id: String,
    reason: TerminalIndexEndReason,
) {
    drop(
        send_notification(
            connection,
            RuntimeMethod::TerminalsIndexEnded,
            &TerminalIndexEndedNotification {
                subscription_id,
                reason,
            },
        )
        .await,
    );
}
