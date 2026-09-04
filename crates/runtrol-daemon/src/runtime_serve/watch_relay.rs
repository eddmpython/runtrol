//! Long-lived provider, event, session-index, and terminal watch relays.

use std::sync::Arc;

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    AppScope, JsonRpcNotification, LaggedNotification, ProtocolRevision, ProviderList,
    ProviderUsageList, ProviderWatchEndReason, ProviderWatchEndedNotification,
    ProvidersChangedNotification, ProvidersUsageChangedNotification, RuntimeErrorKind,
    RuntimeMethod, RuntimeSessionId, SessionIndexChangedNotification, SessionIndexEndReason,
    SessionIndexEndedNotification,
};
use serde::Serialize;
use tokio::sync::watch;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_control::cursor_to_public;
use crate::runtime_inventory::{RuntimeInventoryFailure, RuntimeSessionCatalogue};

use super::authority::refresh_current;
use super::connection_state::{RelayOutcome, Watching};
use super::response::send_notification;
use super::terminal_stream::{relay_terminal, relay_terminal_index};

pub(super) async fn relay_watch(
    connection: &mut Connection,
    watching: Watching,
    sessions: &mut watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    composed: &Composed,
) -> RelayOutcome {
    match watching {
        Watching::Events {
            subscription_id,
            session_id,
            view,
        } => {
            relay_events(connection, subscription_id, session_id, view).await;
            RelayOutcome::CloseConnection
        }
        Watching::SessionIndex {
            subscription_id,
            last,
            authority,
        } => {
            relay_session_index(
                connection,
                sessions,
                composed,
                subscription_id,
                last,
                authority,
            )
            .await;
            RelayOutcome::CloseConnection
        }
        Watching::Providers {
            subscription_id,
            last,
            updates,
            usage,
            authority,
        } => {
            relay_providers(
                connection,
                composed,
                subscription_id,
                last,
                updates,
                usage,
                authority,
            )
            .await;
            RelayOutcome::CloseConnection
        }
        Watching::Terminal(view) => {
            let outcome = relay_terminal(connection, composed, *view).await;
            // The view was dropped with the relay, whether it detached, its connection closed or it stalled:
            // one view fewer is a change of the published index.
            composed.terminals.lock().await.publish_change();
            outcome
        }
        Watching::TerminalIndex {
            subscription_id,
            last,
            updates,
            authority,
        } => {
            relay_terminal_index(
                connection,
                composed,
                subscription_id,
                last,
                updates,
                authority,
            )
            .await;
            RelayOutcome::CloseConnection
        }
        Watching::WindowIndex {
            subscription_id,
            last,
            updates,
        } => {
            super::window_requests::relay_window_index(
                connection,
                composed,
                subscription_id,
                last,
                updates,
            )
            .await;
            RelayOutcome::CloseConnection
        }
        Watching::WindowReveals {
            subscription_id,
            requests,
        } => {
            super::window_requests::relay_window_reveals(connection, subscription_id, requests)
                .await;
            RelayOutcome::CloseConnection
        }
    }
}

async fn relay_providers(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: String,
    mut last: ProviderList,
    mut updates: watch::Receiver<Arc<ProviderList>>,
    mut usage: watch::Receiver<Arc<ProviderUsageList>>,
    authority: AuthorizedIntegration,
) {
    // The usage a subscriber would otherwise have to ask for: sent first, then on every change, so a
    // surface draws the account's position the moment a turn or a probe moves it and never polls.
    let mut last_usage = usage.borrow_and_update().as_ref().clone();
    if send_notification(
        connection,
        RuntimeMethod::ProvidersUsageChanged,
        &ProvidersUsageChangedNotification {
            subscription_id: subscription_id.clone(),
            snapshot: last_usage.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    loop {
        let changed = tokio::select! {
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            changed = updates.changed() => changed,
            usage_changed = usage.changed() => {
                if usage_changed.is_err() {
                    send_provider_watch_end(
                        connection,
                        subscription_id,
                        ProviderWatchEndReason::RuntimeUnavailable,
                    )
                    .await;
                    return;
                }
                let snapshot = usage.borrow_and_update().as_ref().clone();
                if snapshot == last_usage {
                    continue;
                }
                last_usage = snapshot.clone();
                let notification = ProvidersUsageChangedNotification {
                    subscription_id: subscription_id.clone(),
                    snapshot,
                };
                if send_notification(connection, RuntimeMethod::ProvidersUsageChanged, &notification)
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        if changed.is_err() {
            send_provider_watch_end(
                connection,
                subscription_id,
                ProviderWatchEndReason::RuntimeUnavailable,
            )
            .await;
            return;
        }
        match refresh_current(composed, &authority) {
            Ok(current) if current.grant.scopes.contains(&AppScope::ProviderRead) => {}
            Ok(_) => {
                send_provider_watch_end(
                    connection,
                    subscription_id,
                    ProviderWatchEndReason::AuthorityChanged,
                )
                .await;
                return;
            }
            Err(failure) => {
                let reason = if failure.kind == RuntimeErrorKind::IntegrationRevoked {
                    ProviderWatchEndReason::IntegrationRevoked
                } else {
                    ProviderWatchEndReason::AuthorityChanged
                };
                send_provider_watch_end(connection, subscription_id, reason).await;
                return;
            }
        }
        let snapshot = Arc::clone(&updates.borrow_and_update());
        if snapshot.as_ref() == &last {
            continue;
        }
        last = snapshot.as_ref().clone();
        let notification = ProvidersChangedNotification {
            subscription_id: subscription_id.clone(),
            snapshot: last.clone(),
        };
        if send_notification(connection, RuntimeMethod::ProvidersChanged, &notification)
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn send_provider_watch_end(
    connection: &mut Connection,
    subscription_id: String,
    reason: ProviderWatchEndReason,
) {
    let notification = ProviderWatchEndedNotification {
        subscription_id,
        reason,
    };
    drop(
        send_notification(
            connection,
            RuntimeMethod::ProvidersWatchEnded,
            &notification,
        )
        .await,
    );
}

async fn relay_events(
    connection: &mut Connection,
    subscription_id: String,
    session_id: RuntimeSessionId,
    mut view: Box<runtrol_core::SessionView>,
) {
    loop {
        let item = tokio::select! {
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            item = view.recv() => item,
        };
        let Some(item) = item else {
            return;
        };
        match item {
            runtrol_core::WatchItem::Event(event) => {
                let positioned = event.event();
                let next = runtrol_runtime_protocol::EventCursor {
                    stream: view.start().live_at.stream.to_string(),
                    epoch: positioned.epoch,
                    seq: positioned.seq.wrapping_add(1),
                };
                let Ok(wire) = event.wire() else {
                    return;
                };
                let Ok((prefix, suffix)) =
                    event_notification_edges(&subscription_id, &session_id, &next)
                else {
                    return;
                };
                if connection
                    .send_parts(&[&prefix, wire.as_str().as_bytes(), &suffix])
                    .await
                    .is_err()
                {
                    return;
                }
            }
            runtrol_core::WatchItem::Lagged(cursor) => {
                let notification = LaggedNotification {
                    subscription_id,
                    session_id,
                    next_expected: cursor_to_public(cursor),
                };
                drop(
                    send_notification(connection, RuntimeMethod::SessionsLagged, &notification)
                        .await,
                );
                return;
            }
        }
    }
}

async fn relay_session_index(
    connection: &mut Connection,
    sessions: &mut watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    composed: &Composed,
    subscription_id: String,
    mut last: runtrol_runtime_protocol::ManagedSessionList,
    authority: AuthorizedIntegration,
) {
    loop {
        let changed = tokio::select! {
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            changed = sessions.changed() => changed,
        };
        if changed.is_err() {
            send_index_end(
                connection,
                subscription_id,
                SessionIndexEndReason::RuntimeUnavailable,
            )
            .await;
            return;
        }
        let current_authority = match refresh_current(composed, &authority) {
            Ok(authority) if authority.grant.scopes.contains(&AppScope::SessionList) => authority,
            Ok(_) => {
                send_index_end(
                    connection,
                    subscription_id,
                    SessionIndexEndReason::AuthorityChanged,
                )
                .await;
                return;
            }
            Err(failure) => {
                let reason = if failure.kind == RuntimeErrorKind::IntegrationRevoked {
                    SessionIndexEndReason::IntegrationRevoked
                } else {
                    SessionIndexEndReason::AuthorityChanged
                };
                send_index_end(connection, subscription_id, reason).await;
                return;
            }
        };
        let catalogue = Arc::clone(&sessions.borrow_and_update());
        let snapshot = match catalogue.authorized(&current_authority) {
            Ok(snapshot) => snapshot,
            Err(RuntimeInventoryFailure::RootAuthorityChanged) => {
                send_index_end(
                    connection,
                    subscription_id,
                    SessionIndexEndReason::RootDenied,
                )
                .await;
                return;
            }
            Err(
                RuntimeInventoryFailure::Unavailable | RuntimeInventoryFailure::SessionNotFound,
            ) => {
                send_index_end(
                    connection,
                    subscription_id,
                    SessionIndexEndReason::RuntimeUnavailable,
                )
                .await;
                return;
            }
        };
        if snapshot == last {
            continue;
        }
        last = snapshot.clone();
        let notification = SessionIndexChangedNotification {
            subscription_id: subscription_id.clone(),
            snapshot,
        };
        if send_notification(
            connection,
            RuntimeMethod::SessionsIndexChanged,
            &notification,
        )
        .await
        .is_err()
        {
            return;
        }
    }
}

async fn send_index_end(
    connection: &mut Connection,
    subscription_id: String,
    reason: SessionIndexEndReason,
) {
    let notification = SessionIndexEndedNotification {
        subscription_id,
        reason,
    };
    drop(send_notification(connection, RuntimeMethod::SessionsIndexEnded, &notification).await);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventNotificationParams<'a> {
    subscription_id: &'a str,
    session_id: &'a RuntimeSessionId,
    event_revision: ProtocolRevision,
    event: (),
    next_expected: &'a runtrol_runtime_protocol::EventCursor,
}

pub(super) fn event_notification_edges(
    subscription_id: &str,
    session_id: &RuntimeSessionId,
    next_expected: &runtrol_runtime_protocol::EventCursor,
) -> Result<(Vec<u8>, Vec<u8>), serde_json::Error> {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: RuntimeMethod::SessionsEvent.to_string(),
        params: serde_json::to_value(EventNotificationParams {
            subscription_id,
            session_id,
            event_revision: runtrol_runtime_protocol::REVISION_2026_08_13,
            event: (),
            next_expected,
        })?,
    };
    let mut encoded = serde_json::to_vec(&notification)?;
    let needle = b"\"event\":null";
    let Some(start) = encoded
        .windows(needle.len())
        .position(|window| window == needle)
    else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "event placeholder disappeared",
        )));
    };
    let value_start = start.saturating_add(b"\"event\":".len());
    let suffix = encoded.split_off(value_start.saturating_add(4));
    encoded.truncate(value_start);
    Ok((encoded, suffix))
}
