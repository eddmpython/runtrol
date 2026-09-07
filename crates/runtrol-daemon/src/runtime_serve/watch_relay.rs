//! Long-lived provider, event, session-index, and terminal watch relays.

use std::sync::Arc;

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    AppScope, JsonRpcNotification, LaggedNotification, NativeActivityObservation, ProtocolRevision,
    ProviderList, ProviderUsageList, ProviderWatchEndReason, ProviderWatchEndedNotification,
    ProvidersChangedNotification, ProvidersNativeActivityChangedNotification,
    ProvidersUsageChangedNotification, RuntimeErrorKind, RuntimeMethod, RuntimeSessionId,
    SessionIndexChangedNotification, SessionIndexEndReason, SessionIndexEndedNotification,
};
use serde::Serialize;
use tokio::sync::watch;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_control::cursor_to_public;
use crate::runtime_inventory::{RuntimeInventoryFailure, RuntimeSessionCatalogue};

use super::authority::{refresh_current, refresh_current_in_place};
use super::connection_state::{RelayOutcome, Watching};
use super::response::send_notification;
use super::terminal_stream::{relay_terminal, relay_terminal_index};

#[cfg(test)]
#[path = "tests/provider_watch.rs"]
mod provider_watch_tests;

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
            native,
            authority,
        } => {
            relay_providers(
                connection,
                composed,
                subscription_id,
                last,
                ProviderStreams {
                    updates,
                    usage,
                    native,
                },
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
        Watching::WindowInput(receiver) => {
            super::window_input::relay_input(connection, composed, receiver).await;
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

struct ProviderStreams {
    updates: watch::Receiver<Arc<ProviderList>>,
    usage: watch::Receiver<Arc<ProviderUsageList>>,
    native: Option<watch::Receiver<Arc<crate::native_activity::NativeSnapshot>>>,
}

enum ProviderWake {
    Authority,
    Inventory,
    Usage,
    Native,
}

async fn relay_providers(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: String,
    mut last: ProviderList,
    mut streams: ProviderStreams,
    mut authority: AuthorizedIntegration,
) {
    let mut primary_authority_updates = composed.integration_authority.subscribe();
    let mut draining_authority_updates = composed.generation_authority.subscribe();
    // Subscriptions precede admission and initial publication, including an awaited native projection.
    let Ok(mut published) = initialize_provider_frames(
        connection,
        composed,
        &subscription_id,
        &mut streams,
        &mut authority,
    )
    .await
    else {
        return;
    };
    let ProviderStreams {
        mut updates,
        mut usage,
        mut native,
    } = streams;
    let mut authority_tick = tokio::time::interval(std::time::Duration::from_millis(500));
    authority_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let wake = tokio::select! {
            biased;
            changed = primary_authority_updates.changed() => changed.map(|()| ProviderWake::Authority),
            changed = draining_authority_updates.changed() => changed.map(|()| ProviderWake::Authority),
            _ = authority_tick.tick(), if composed.draining.load(std::sync::atomic::Ordering::Acquire) => Ok(ProviderWake::Authority),
            peer = connection.recv() => {
                drop(peer);
                return;
            }
            changed = updates.changed() => changed.map(|()| ProviderWake::Inventory),
            changed = usage.changed() => changed.map(|()| ProviderWake::Usage),
            changed = native_changed(&mut native) => changed.map(|()| ProviderWake::Native),
        };
        let Ok(wake) = wake else {
            send_provider_watch_end(
                connection,
                subscription_id,
                ProviderWatchEndReason::RuntimeUnavailable,
            )
            .await;
            return;
        };
        // Usage and inventory share the same current authority witness. Adjacent approved widenings must
        // advance it, while revocation, key replacement and scope/root shrink retire the subscription.
        if let Err(reason) = refresh_provider_authority(composed, &mut authority, native.is_some())
        {
            send_provider_watch_end(connection, subscription_id, reason).await;
            return;
        }
        match wake {
            ProviderWake::Authority => continue,
            ProviderWake::Native => {
                if let Some(native) = &mut native
                    && send_native_snapshot(
                        connection,
                        composed,
                        &subscription_id,
                        native,
                        &mut authority,
                        &mut published.native,
                    )
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
            ProviderWake::Usage => {
                let snapshot = usage.borrow_and_update().as_ref().clone();
                if snapshot == published.usage {
                    continue;
                }
                published.usage = snapshot.clone();
                if send_provider_usage(connection, &subscription_id, snapshot)
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
            ProviderWake::Inventory => {}
        }
        let snapshot = Arc::clone(&updates.borrow_and_update());
        if snapshot.as_ref() == &last {
            continue;
        }
        last = snapshot.as_ref().clone();
        if send_provider_inventory(connection, &subscription_id, last.clone())
            .await
            .is_err()
        {
            return;
        }
    }
}

struct ProviderFrames {
    usage: ProviderUsageList,
    native: Option<Vec<NativeActivityObservation>>,
}

async fn initialize_provider_frames(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: &str,
    streams: &mut ProviderStreams,
    authority: &mut AuthorizedIntegration,
) -> Result<ProviderFrames, ()> {
    if let Err(reason) = refresh_provider_authority(composed, authority, streams.native.is_some()) {
        send_provider_watch_end(connection, subscription_id.to_owned(), reason).await;
        return Err(());
    }
    let usage = streams.usage.borrow_and_update().as_ref().clone();
    send_provider_usage(connection, subscription_id, usage.clone()).await?;
    let mut published = ProviderFrames {
        usage,
        native: None,
    };
    if let Some(native) = &mut streams.native {
        send_native_snapshot(
            connection,
            composed,
            subscription_id,
            native,
            authority,
            &mut published.native,
        )
        .await?;
    }
    Ok(published)
}

async fn send_provider_inventory(
    connection: &mut Connection,
    subscription_id: &str,
    snapshot: ProviderList,
) -> Result<(), ()> {
    send_notification(
        connection,
        RuntimeMethod::ProvidersChanged,
        &ProvidersChangedNotification {
            subscription_id: subscription_id.to_owned(),
            snapshot,
        },
    )
    .await
    .map_err(|_| ())
}

async fn send_provider_usage(
    connection: &mut Connection,
    subscription_id: &str,
    snapshot: ProviderUsageList,
) -> Result<(), ()> {
    send_notification(
        connection,
        RuntimeMethod::ProvidersUsageChanged,
        &ProvidersUsageChangedNotification {
            subscription_id: subscription_id.to_owned(),
            snapshot,
        },
    )
    .await
    .map_err(|_| ())
}

async fn send_native_snapshot(
    connection: &mut Connection,
    composed: &Composed,
    subscription_id: &str,
    updates: &mut watch::Receiver<Arc<crate::native_activity::NativeSnapshot>>,
    authority: &mut AuthorizedIntegration,
    last: &mut Option<Vec<NativeActivityObservation>>,
) -> Result<(), ()> {
    let snapshot = Arc::clone(&updates.borrow_and_update());
    let snapshot = native_snapshot(composed, &snapshot).await;
    // Both initial publication and later changes share the same after-await authority witness.
    if let Err(reason) = refresh_provider_authority(composed, authority, true) {
        send_provider_watch_end(connection, subscription_id.to_owned(), reason).await;
        return Err(());
    }
    if last.as_ref() == Some(&snapshot) {
        return Ok(());
    }
    *last = Some(snapshot.clone());
    send_notification(
        connection,
        RuntimeMethod::ProvidersNativeActivityChanged,
        &ProvidersNativeActivityChangedNotification {
            subscription_id: subscription_id.to_owned(),
            snapshot,
        },
    )
    .await
    .map_err(|_| ())
}

async fn native_changed(
    native: &mut Option<watch::Receiver<Arc<crate::native_activity::NativeSnapshot>>>,
) -> Result<(), watch::error::RecvError> {
    match native {
        Some(native) => native.changed().await,
        None => std::future::pending().await,
    }
}

async fn native_snapshot(
    composed: &Composed,
    snapshot: &crate::native_activity::NativeSnapshot,
) -> Vec<NativeActivityObservation> {
    let mut public = Vec::with_capacity(snapshot.len());
    for (provider, entry) in snapshot {
        let provider_id = runtrol_runtime_protocol::ProviderId::new(provider.as_str());
        let Some(observed) = &entry.observation else {
            public.push(NativeActivityObservation::Unavailable { provider_id });
            continue;
        };
        let activity = &observed.activity;
        public.push(NativeActivityObservation::Observed {
            activity: runtrol_runtime_protocol::NativeActivity {
                provider_id,
                live: activity.live.iter().map(ToString::to_string).collect(),
                active: activity.active.iter().map(ToString::to_string).collect(),
                attachable: super::provider_requests::attachable_native_sessions(activity),
                focusable: super::provider_requests::focusable_native_sessions(composed, *provider)
                    .await,
            },
            catalogue_revision: entry.catalogue_revision(),
            unknown_activity: observed
                .unknown_activity
                .iter()
                .map(ToString::to_string)
                .collect(),
        });
    }
    public
}

fn refresh_provider_authority(
    composed: &Composed,
    authority: &mut AuthorizedIntegration,
    native: bool,
) -> Result<(), ProviderWatchEndReason> {
    refresh_current_in_place(composed, authority).map_err(|failure| match failure.kind {
        RuntimeErrorKind::IntegrationRevoked => ProviderWatchEndReason::IntegrationRevoked,
        RuntimeErrorKind::RuntimeUnavailable => ProviderWatchEndReason::RuntimeUnavailable,
        _ => ProviderWatchEndReason::AuthorityChanged,
    })?;
    if !authority.grant.scopes.contains(&AppScope::ProviderRead)
        || native
            && !authority
                .grant
                .scopes
                .contains(&AppScope::SessionNativeDiscover)
    {
        return Err(ProviderWatchEndReason::AuthorityChanged);
    }
    Ok(())
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
