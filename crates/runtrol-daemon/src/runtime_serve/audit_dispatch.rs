//! Audit admission and completion around public JSON-RPC dispatch.

use std::sync::Arc;

use runtrol_runtime_protocol::{
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, ProviderList, ProviderUsageList, RuntimeErrorKind,
    RuntimeMethod,
};
use runtrol_store::IntegrationAuditOutcome;
use tokio::sync::{mpsc, watch};

use crate::Composed;
use crate::runtime_control::{RuntimeAsked, RuntimeReturned};
use crate::runtime_inventory::RuntimeSessionCatalogue;
use crate::runtime_native_sessions::NativeCursorCodec;

use super::authority::required_scope;
use super::connection_state::{PublicAuthority, PublicState};
use super::dispatch::dispatch_public;
use super::response::{Answer, failure_response};

#[expect(
    clippy::too_many_arguments,
    reason = "one audited request keeps connection authority, discovery admission, session state, and owner channels explicit"
)]
pub(super) async fn answer(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Arc<Composed>,
    audit: &crate::runtime_audit::AuditJournal,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &NativeCursorCodec,
    provider_updates: &watch::Sender<Arc<ProviderList>>,
    providers: &ProviderList,
    sessions: &RuntimeSessionCatalogue,
    usage: &ProviderUsageList,
    usage_updates: &watch::Receiver<Arc<ProviderUsageList>>,
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    request: JsonRpcRequest,
) -> Answer {
    let id = request.id;
    let Ok(audit_admission) = audit.try_admit() else {
        return audit_unavailable(id);
    };
    if request.jsonrpc != "2.0" {
        if audit
            .deny_structural(
                &audit_admission,
                "runtime/invalidEnvelope",
                RuntimeErrorKind::InvalidRequest.as_str(),
            )
            .await
            .is_err()
        {
            return audit_unavailable(id);
        }
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "JSON-RPC version must be 2.0",
        );
    }
    let Ok(method) = request.method.parse::<RuntimeMethod>() else {
        if audit
            .deny_structural(
                &audit_admission,
                "runtime/unknownMethod",
                RuntimeErrorKind::MethodNotFound.as_str(),
            )
            .await
            .is_err()
        {
            return audit_unavailable(id);
        }
        return Answer::plain(
            id,
            RuntimeErrorKind::MethodNotFound,
            "the public Runtime method does not exist",
        );
    };
    let audit_response_id = id.clone();
    let scope = required_scope(method);
    let (integration, key_generation) = audit_identity(state);
    // The observed mirror's byte feed is data, not an authority event: the decision to mirror that terminal was
    // audited at `windows/mirrorOpen`, and a provider that redraws its screen would otherwise write two durable audit
    // rows per redraw and crowd out the events this journal exists for. Terminal output is not audited per byte
    // either. Every other method, including the open and the end, stays audited.
    let audited = !matches!(method, RuntimeMethod::WindowsMirrorOutput);
    if audited
        && audit
            .attempt(
                &audit_admission,
                crate::runtime_audit::AuditContext::new(integration, key_generation, method, scope),
            )
            .await
            .is_err()
    {
        return audit_unavailable(id);
    }
    let answered = dispatch_public(
        state,
        instance_id,
        composed,
        discovering,
        native_cursors,
        provider_updates,
        providers,
        sessions,
        usage,
        usage_updates,
        asking,
        returning,
        method,
        id,
        request.params,
    )
    .await;
    let (outcome, reason) = match &answered.response {
        JsonRpcResponse::Success(_) => (IntegrationAuditOutcome::Allowed, "allowed"),
        JsonRpcResponse::Error(error) => {
            (IntegrationAuditOutcome::Denied, error.error.code.as_str())
        }
    };
    if !audited {
        return answered;
    }
    let (integration, key_generation) = audit_identity(state);
    if audit
        .finish(
            &audit_admission,
            crate::runtime_audit::AuditContext::new(integration, key_generation, method, scope),
            outcome,
            reason,
        )
        .await
        .is_err()
    {
        return audit_unavailable(audit_response_id);
    }
    answered
}

fn audit_identity(state: &PublicState) -> (Option<runtrol_store::IntegrationKey>, Option<u64>) {
    let authority = match state {
        PublicState::Negotiated { authority, .. } | PublicState::Ready { authority, .. } => {
            authority
        }
        PublicState::Fresh { .. } => return (None, None),
    };
    match authority {
        PublicAuthority::Authorized(authorized) => {
            (Some(authorized.key), Some(authorized.grant.key_generation))
        }
        PublicAuthority::Anonymous | PublicAuthority::Pending(_) => (None, None),
    }
}

fn audit_unavailable(id: JsonRpcId) -> Answer {
    Answer {
        response: failure_response(
            id,
            RuntimeErrorKind::Internal,
            "Runtime authorization audit storage is unavailable",
        ),
        close: true,
        watching: None,
    }
}
