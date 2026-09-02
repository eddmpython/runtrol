//! Public Runtime connection lifecycle and watch-mode transitions.

use std::sync::Arc;

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    JsonRpcNotification, JsonRpcRequest, ProviderList, ProviderUsageList, RuntimeMethod,
};
use tokio::sync::{mpsc, watch};

use crate::Composed;
use crate::runtime_auth::challenge;
use crate::runtime_control::{RuntimeAsked, RuntimeReturned};
use crate::runtime_inventory::RuntimeSessionCatalogue;
use crate::runtime_native_sessions::NativeCursorCodec;

use super::audit_dispatch::answer;
use super::connection_state::{PublicState, RelayOutcome};
use super::provider_requests::{
    method_needs_provider_refresh, schedule_provider_inventory_refresh,
};
use super::response::{EmptyParams, send_notification, send_response};
use super::watch_relay::relay_watch;
use crate::window_registry::ConnectionToken;

/// Serve one public connection until it closes or violates the public frame contract, then take with it
/// whatever it registered.
#[expect(
    clippy::too_many_arguments,
    reason = "one connection keeps endpoint identity, discovery authority, catalogue snapshots, and owner channels explicit"
)]
pub(crate) async fn serve_connection(
    connection: Connection,
    instance_id: String,
    composed: Arc<Composed>,
    audit: crate::runtime_audit::AuditJournal,
    discovering: Arc<crate::serve::DiscoveryGates>,
    native_cursors: Arc<NativeCursorCodec>,
    providers: watch::Sender<Arc<ProviderList>>,
    sessions: watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    account_gauges: watch::Receiver<Arc<ProviderUsageList>>,
    asking: mpsc::Sender<Box<RuntimeAsked>>,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) {
    let token = ConnectionToken::next();
    serve_requests(
        connection,
        token,
        instance_id,
        Arc::clone(&composed),
        audit,
        discovering,
        native_cursors,
        providers,
        sessions,
        account_gauges,
        asking,
        returning,
    )
    .await;
    // A window's registration lives exactly as long as this connection.
    composed.windows.forget_connection(token).await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "one connection binds every shared publisher it may serve; bundling them would only rename the list"
)]
async fn serve_requests(
    mut connection: Connection,
    token: ConnectionToken,
    instance_id: String,
    composed: Arc<Composed>,
    audit: crate::runtime_audit::AuditJournal,
    discovering: Arc<crate::serve::DiscoveryGates>,
    native_cursors: Arc<NativeCursorCodec>,
    providers: watch::Sender<Arc<ProviderList>>,
    mut sessions: watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    mut account_gauges: watch::Receiver<Arc<ProviderUsageList>>,
    asking: mpsc::Sender<Box<RuntimeAsked>>,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) {
    let Ok(challenge) = challenge(&instance_id) else {
        return;
    };
    if send_notification(&mut connection, RuntimeMethod::Challenge, &challenge)
        .await
        .is_err()
    {
        return;
    }
    let mut state = PublicState::Fresh { challenge, token };
    loop {
        let Ok(Some(payload)) = connection.recv().await else {
            return;
        };
        if matches!(state, PublicState::Negotiated { .. }) {
            let Ok(notification) = serde_json::from_slice::<JsonRpcNotification>(&payload) else {
                return;
            };
            if notification.jsonrpc != "2.0"
                || notification.method.parse::<RuntimeMethod>() != Ok(RuntimeMethod::Initialized)
                || serde_json::from_value::<EmptyParams>(notification.params).is_err()
            {
                return;
            }
            state = match state {
                PublicState::Negotiated {
                    context,
                    authority,
                    token,
                } => PublicState::Ready {
                    context,
                    authority,
                    token,
                },
                PublicState::Fresh { .. } | PublicState::Ready { .. } => return,
            };
            continue;
        }

        let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&payload) else {
            return;
        };
        let refresh_providers = request
            .method
            .parse::<RuntimeMethod>()
            .is_ok_and(method_needs_provider_refresh);
        if refresh_providers {
            schedule_provider_inventory_refresh(providers.clone(), Arc::clone(&composed));
        }
        let catalogue = Arc::clone(&sessions.borrow_and_update());
        let provider_catalogue = Arc::clone(&providers.borrow());
        let usage = Arc::clone(&account_gauges.borrow_and_update());
        let answered = answer(
            &mut state,
            &instance_id,
            &composed,
            &audit,
            &discovering,
            &native_cursors,
            &providers,
            &provider_catalogue,
            &catalogue,
            &usage,
            &account_gauges,
            &asking,
            &returning,
            request,
        )
        .await;
        if refresh_providers {
            schedule_provider_inventory_refresh(providers.clone(), Arc::clone(&composed));
        }
        if send_response(&mut connection, &answered.response)
            .await
            .is_err()
            || answered.close
        {
            return;
        }
        if let Some(watching) = answered.watching {
            if relay_watch(&mut connection, watching, &mut sessions, composed.as_ref()).await
                == RelayOutcome::ResumeRequests
            {
                continue;
            }
            return;
        }
    }
}
