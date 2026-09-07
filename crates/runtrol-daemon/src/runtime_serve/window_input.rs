//! Dedicated owner input relay: bounded body-free offers, single claims, and structural receipts.

use std::collections::BTreeMap;
use std::time::Instant;

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    JsonRpcId, JsonRpcRequest, RuntimeErrorKind, RuntimeMethod, WatchWindowInputParams,
    WatchWindowInputResult, WindowClaimInputParams, WindowIndexEndReason,
    WindowInputEndedNotification, WindowInputOfferedNotification, WindowInputReceiptParams,
};

use super::authority::authorized_scopes;
use super::connection_state::{PublicState, Watching};
use super::response::{
    Answer, EmptyResult, failure_response, random_subscription_id, send_notification,
    send_response, success,
};
use crate::Composed;
use crate::runtime_terminal::owner_input::{DELIVERY_DEADLINE, OwnerOffer};
use crate::window_registry::input::InputReceiver;

pub(super) async fn watch_input(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let authority = match authorized_scopes(
        state,
        composed,
        &[runtrol_runtime_protocol::AppScope::SessionInputWrite],
    ) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let Ok(params) = serde_json::from_value::<WatchWindowInputParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "owner input watch parameters are invalid",
        );
    };
    let Ok(subscription_id) = random_subscription_id() else {
        return Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "could not allocate an owner input subscription",
        );
    };
    let changes = composed.terminals.lock().await.change_sender();
    match composed
        .windows
        .watch_input(authority, params, subscription_id.clone(), changes)
        .await
    {
        Ok(receiver) => {
            let result = WatchWindowInputResult {
                subscription_id,
                max_pending_offers: crate::terminal_surface::MAX_HOSTED_TERMINALS,
            };
            let mut answer = Answer::success(id, &result);
            answer.watching = Some(Watching::WindowInput(receiver));
            answer
        }
        Err(failure) => Answer::plain(id, failure.kind, failure.message),
    }
}

pub(super) async fn relay_input(
    connection: &mut Connection,
    composed: &Composed,
    mut receiver: InputReceiver,
) {
    let mut pending = BTreeMap::<u64, OwnerOffer>::new();
    let mut windows = composed.windows.changes();
    let mut primary = composed.integration_authority.subscribe();
    let mut draining = composed.generation_authority.subscribe();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        pending.retain(|_, offer| !offer.expired());
        if !composed.windows.input_is_current(&receiver).await
            || crate::runtime_terminal::validate_input_grant(composed, &receiver.authority).is_err()
        {
            let ended = WindowInputEndedNotification {
                subscription_id: receiver.subscription_id.clone(),
                reason: WindowIndexEndReason::AuthorityChanged,
            };
            // ok: Retirement drops every pending body even if the disconnected owner cannot receive its reason.
            let _ended = tokio::time::timeout(
                DELIVERY_DEADLINE,
                send_notification(connection, RuntimeMethod::WindowsInputEnded, &ended),
            )
            .await;
            return;
        }
        let deadline = pending.values().map(|offer| offer.deadline).min();
        tokio::select! {
            biased;
            changed = primary.changed() => { if changed.is_err() { return; } }
            changed = draining.changed() => { if changed.is_err() { return; } }
            changed = windows.changed() => { if changed.is_err() { return; } }
            _ = tick.tick(), if composed.draining.load(std::sync::atomic::Ordering::Acquire) => {}
            () = until_deadline(deadline) => {}
            payload = connection.recv() => {
                let Ok(Some(payload)) = payload else { return; };
                let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&payload) else { return; };
                let response = owner_request(composed, &receiver.subscription_id, &mut pending, request).await;
                if !matches!(tokio::time::timeout(DELIVERY_DEADLINE, send_response(connection, &response)).await, Ok(Ok(()))) { return; }
            }
            offer = receiver.requests.recv() => {
                let Some(offer) = offer else { return; };
                if offer.expired() { continue; }
                if pending.len() >= crate::terminal_surface::MAX_HOSTED_TERMINALS { return; }
                let notice = WindowInputOfferedNotification { subscription_id: receiver.subscription_id.clone(), sequence: offer.sequence, binding: offer.binding.clone() };
                pending.insert(offer.sequence, offer);
                if !matches!(tokio::time::timeout(DELIVERY_DEADLINE, send_notification(connection, RuntimeMethod::WindowsInputOffered, &notice)).await, Ok(Ok(()))) { return; }
            }
        }
    }
}

async fn until_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => std::future::pending().await,
    }
}

async fn owner_request(
    composed: &Composed,
    subscription: &str,
    pending: &mut BTreeMap<u64, OwnerOffer>,
    request: JsonRpcRequest,
) -> runtrol_runtime_protocol::JsonRpcResponse {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return failure_response(
            id,
            RuntimeErrorKind::InvalidRequest,
            "JSON-RPC version must be 2.0",
        );
    }
    match request.method.parse::<RuntimeMethod>() {
        Ok(RuntimeMethod::WindowsClaimInput) => {
            let Ok(params) = serde_json::from_value::<WindowClaimInputParams>(request.params)
            else {
                return failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "owner input claim parameters are invalid",
                );
            };
            if params.subscription_id != subscription {
                return failure_response(
                    id,
                    RuntimeErrorKind::ScopeDenied,
                    "the input receiver belongs to another connection",
                );
            }
            let Some(offer) = pending.get_mut(&params.sequence) else {
                return failure_response(
                    id,
                    RuntimeErrorKind::OutcomeUnknown,
                    "the input offer is no longer claimable",
                );
            };
            match offer.claim(composed, subscription).await {
                Ok(claim) => success(id, &claim),
                Err(failure) => {
                    pending.remove(&params.sequence);
                    failure_response(id, failure.kind, failure.message)
                }
            }
        }
        Ok(RuntimeMethod::WindowsInputReceipt) => {
            let Ok(params) = serde_json::from_value::<WindowInputReceiptParams>(request.params)
            else {
                return failure_response(
                    id,
                    RuntimeErrorKind::InvalidRequest,
                    "owner input receipt parameters are invalid",
                );
            };
            if params.subscription_id != subscription {
                return failure_response(
                    id,
                    RuntimeErrorKind::ScopeDenied,
                    "the input receiver belongs to another connection",
                );
            }
            let Some(offer) = pending.remove(&params.sequence) else {
                return failure_response(
                    id,
                    RuntimeErrorKind::OutcomeUnknown,
                    "the input receipt has no pending delivery",
                );
            };
            offer.finish(composed, &params).await;
            success(id, &EmptyResult {})
        }
        _ => failure_response(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the owner receiver accepts only claim and receipt operations",
        ),
    }
}
