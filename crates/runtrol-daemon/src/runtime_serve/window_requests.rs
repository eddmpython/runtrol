//! The window registry over the public wire: a window registers and updates on its own connection, every
//! reader lists or watches the index, and a window feeds the mirrors of the terminals it observes.

use std::sync::Arc;

use base64ct::{Base64, Encoding as _};
use runtrol_ipc::transport::Connection;
use runtrol_provider::TerminalId;
use runtrol_runtime_protocol::{
    AppScope, JsonRpcId, ListWindowsParams, RuntimeErrorKind, RuntimeMethod,
    WatchWindowIndexParams, WatchWindowIndexResult, WindowIndexChangedNotification,
    WindowIndexEndReason, WindowIndexEndedNotification, WindowIndexSnapshot, WindowMirrorEndParams,
    WindowMirrorOpenParams, WindowMirrorOpened, WindowMirrorOutputParams, WindowRegisterParams,
    WindowUpdateParams,
};
use tokio::sync::watch;

use super::authority::authorized_scopes;
use super::connection_state::PublicState;
use super::response::{Answer, EmptyResult, random_subscription_id, send_notification};
use crate::Composed;
use crate::terminal_surface::TerminalOpenError;
use crate::window_registry::{ConnectionToken, WindowRegistryFailure};

/// The largest chunk a window may feed at once: the public terminal input ceiling, applied to output too.
const MAX_MIRROR_CHUNK_BYTES: usize = 64 * 1024;

pub(super) async fn window_operation(
    state: &mut PublicState,
    composed: &Arc<Composed>,
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
        RuntimeMethod::WindowsMirrorOpen => mirror_open(token, composed, id, params).await,
        RuntimeMethod::WindowsMirrorOutput => mirror_output(token, composed, id, params).await,
        RuntimeMethod::WindowsMirrorEnd => mirror_end(token, composed, id, params).await,
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

async fn mirror_open(
    token: ConnectionToken,
    composed: &Arc<Composed>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<WindowMirrorOpenParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "mirror open parameters are invalid",
        );
    };
    let Some(window_session_id) = composed.windows.session_id_of(token).await else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the connection registered no window",
        );
    };
    match crate::terminal_surface::open_observed_mirror(composed, token, window_session_id, params)
        .await
    {
        Ok(terminal_id) => match terminal_id.to_string().parse() {
            Ok(terminal_id) => Answer::success(id, &WindowMirrorOpened { terminal_id }),
            Err(_) => Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not project a terminal identity",
            ),
        },
        Err(error) => mirror_refused(id, &error),
    }
}

async fn mirror_output(
    token: ConnectionToken,
    composed: &Arc<Composed>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<WindowMirrorOutputParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "mirror output parameters are invalid",
        );
    };
    let Ok(bytes) = Base64::decode_vec(&params.bytes_base64) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "mirror output is not base64",
        );
    };
    if bytes.len() > MAX_MIRROR_CHUNK_BYTES {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "mirror output exceeds the 64 KiB chunk bound",
        );
    }
    let terminal_id = match local_terminal_id(&params.terminal_id, &id) {
        Ok(terminal_id) => terminal_id,
        Err(refusal) => return *refusal,
    };
    match crate::terminal_surface::feed_observed_mirror(composed, token, terminal_id, bytes).await {
        Ok(()) => Answer::success(id, &EmptyResult {}),
        Err(error) => mirror_refused(id, &error),
    }
}

async fn mirror_end(
    token: ConnectionToken,
    composed: &Arc<Composed>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<WindowMirrorEndParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "mirror end parameters are invalid",
        );
    };
    let terminal_id = match local_terminal_id(&params.terminal_id, &id) {
        Ok(terminal_id) => terminal_id,
        Err(refusal) => return *refusal,
    };
    match crate::terminal_surface::end_observed_mirror(
        composed,
        token,
        terminal_id,
        params.exit_code,
    )
    .await
    {
        Ok(()) => Answer::success(id, &EmptyResult {}),
        Err(error) => mirror_refused(id, &error),
    }
}

fn local_terminal_id(
    public: &runtrol_runtime_protocol::RuntimeTerminalId,
    id: &JsonRpcId,
) -> Result<TerminalId, Box<Answer>> {
    public.to_string().parse().map_err(|_| {
        Box::new(Answer::plain(
            id.clone(),
            RuntimeErrorKind::TerminalNotFound,
            "the terminal identity is invalid",
        ))
    })
}

fn mirror_refused(id: JsonRpcId, error: &TerminalOpenError) -> Answer {
    let kind = match error {
        TerminalOpenError::NoRoom { .. } => RuntimeErrorKind::ResourceExhausted,
        TerminalOpenError::AlreadyBrokered => RuntimeErrorKind::TerminalAlreadyLive,
        TerminalOpenError::NotFedByCaller => RuntimeErrorKind::TerminalNotFound,
        TerminalOpenError::Provider(_) | TerminalOpenError::Claim(_) => {
            RuntimeErrorKind::InvalidRequest
        }
    };
    Answer::plain(id, kind, &error.to_string())
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
