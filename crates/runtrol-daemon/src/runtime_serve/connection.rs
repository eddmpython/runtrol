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

/// Serve one public connection until it closes or violates the public frame contract.
#[expect(
    clippy::too_many_arguments,
    reason = "one connection keeps endpoint identity, discovery authority, catalogue snapshots, and owner channels explicit"
)]
pub(crate) async fn serve_connection(
    mut connection: Connection,
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
    let mut state = PublicState::Fresh { challenge };
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
                PublicState::Negotiated { context, authority } => {
                    PublicState::Ready { context, authority }
                }
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
