//! The window registry over the public wire: a window registers and updates on its own connection, every
//! reader lists or watches the index.

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    AppScope, JsonRpcId, ListWindowsParams, RuntimeErrorKind, RuntimeMethod,
    WatchWindowIndexParams, WatchWindowIndexResult, WindowIndexChangedNotification,
    WindowIndexEndReason, WindowIndexEndedNotification, WindowIndexSnapshot, WindowRegisterParams,
    WindowUpdateParams,
};
use tokio::sync::watch;

use super::authority::authorized_scopes;
use super::connection_state::PublicState;
use super::response::{Answer, EmptyResult, random_subscription_id, send_notification};
use crate::Composed;
use crate::window_registry::WindowRegistryFailure;

pub(super) async fn window_operation(
    state: &mut PublicState,
    composed: &Composed,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if let Err(failure) = authorized_scopes(state, composed, &[AppScope::SessionList]) {
        return Answer::failure(id, failure);
    }
    let token = state.token();
    match method {
        RuntimeMethod::WindowsRegister => {
            let Ok(params) = serde_json::from_value::<WindowRegisterParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "window registration parameters are invalid",
                );
            };
            match composed.windows.register(token, params).await {
                Ok(registration) => Answer::success(id, &registration),
                Err(failure) => refused(id, failure),
            }
        }
        RuntimeMethod::WindowsUpdate => {
            let Ok(params) = serde_json::from_value::<WindowUpdateParams>(params) else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "window update parameters are invalid",
                );
            };
            match composed.windows.update(token, params).await {
                Ok(()) => Answer::success(id, &EmptyResult {}),
                Err(failure) => refused(id, failure),
            }
        }
        RuntimeMethod::WindowsList => {
            if serde_json::from_value::<ListWindowsParams>(params).is_err() {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "window list parameters are invalid",
                );
            }
            Answer::success(id, &composed.windows.snapshot().await)
        }
        RuntimeMethod::WindowsWatchIndex => {
            if serde_json::from_value::<WatchWindowIndexParams>(params).is_err() {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "window index watch parameters are invalid",
                );
            }
            let updates = composed.windows.changes();
            let snapshot = composed.windows.snapshot().await;
            let Ok(subscription_id) = random_subscription_id() else {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::Internal,
                    "Runtime could not allocate a window index subscription",
                );
            };
            let result = WatchWindowIndexResult {
                subscription_id,
                snapshot,
            };
            Answer::watching_window_index(id, &result, updates)
        }
        _ => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the method is not a window operation",
        ),
    }
}

fn refused(id: JsonRpcId, failure: WindowRegistryFailure) -> Answer {
    Answer::plain(id, failure.kind, failure.message)
}

/// Send the window index on every change until the subscriber goes away or the registry does.
pub(super) async fn relay_window_index(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: String,
    mut last: WindowIndexSnapshot,
    mut updates: watch::Receiver<u64>,
) {
    loop {
        let changed = tokio::select! {
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            changed = updates.changed() => changed,
        };
        if changed.is_err() {
            drop(
                send_notification(
                    connection,
                    RuntimeMethod::WindowsIndexEnded,
                    &WindowIndexEndedNotification {
                        subscription_id,
                        reason: WindowIndexEndReason::RuntimeUnavailable,
                    },
                )
                .await,
            );
            return;
        }
        let snapshot = composed.windows.snapshot().await;
        if snapshot == last {
            continue;
        }
        last = snapshot.clone();
        if send_notification(
            connection,
            RuntimeMethod::WindowsIndexChanged,
            &WindowIndexChangedNotification {
                subscription_id: subscription_id.clone(),
                snapshot,
            },
        )
        .await
        .is_err()
        {
            return;
        }
    }
}
