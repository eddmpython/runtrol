//! Separate public Runtime listener with challenge-bound integration authority.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use runtrol_core::WorkspaceClaim;
use runtrol_ipc::transport::Connection;
use runtrol_provider::{CloseMode, Disposition, OpenIntent, WorkspaceAccess};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, ControlLeaseParams, ErrorResponse,
    FINALIZED_REVISIONS, GetProviderCapabilitiesParams, GetSessionParams, InitializeParams,
    InitializeResult, JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    LaggedNotification, ListModelsParams, ListNativeSessionsParams, MAX_MODEL_SELECTION_BYTES,
    MAX_NATIVE_ADOPTION_TOKEN_BYTES, MAX_NATIVE_PUBLIC_CURSOR_BYTES, MAX_PAGE_ITEMS,
    MAX_REVISION_OFFERS, ProtocolRevision, ProviderCapabilityAvailability,
    ProviderCapabilityObservation, ProviderCapabilityProvenance, RequestEnrollmentParams,
    ResumeSessionParams, RuntimeCapabilities, RuntimeError, RuntimeErrorKind, RuntimeInstance,
    RuntimeLimits, RuntimeMethod, RuntimeModelCatalog, RuntimeModelChoice,
    RuntimeProviderCapabilities, RuntimeReasoningChoice, RuntimeSessionId, SessionWorkspaceAccess,
    StartSessionParams, SubmitInputParams, SuccessResponse, WatchEnrollmentParams,
    WatchEventsParams, WatchEventsResult, negotiate,
};
use runtrol_store::EnrollmentKey;
use runtrol_store::IntegrationAuditOutcome;
use serde::Serialize;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use crate::Composed;
use crate::runtime_auth::{
    AuthorizationFailure, AuthorizedIntegration, ClientContext, authenticate, challenge,
    enrollment_decision, refresh, request_enrollment,
};
use crate::runtime_control::{
    RuntimeAgentGuard, RuntimeAsked, RuntimeControlFailure, RuntimeControlReply,
    RuntimeControlRequest, RuntimeOpenCompletion, RuntimeOpenGuard, RuntimeOpenRequest,
    RuntimeReturned, cursor_to_public,
};
use crate::runtime_inventory::{
    RuntimeInventoryFailure, RuntimeSessionCatalogue, authorized_roots, authorized_workspace,
};
use crate::runtime_native_sessions::{NativeCursorCodec, NativeCursorFailure};

/// Serve one public connection until it closes or violates the public frame contract.
#[expect(
    clippy::too_many_arguments,
    reason = "one connection keeps endpoint identity, discovery authority, catalogue snapshots, and owner channels explicit"
)]
pub(crate) async fn serve_connection(
    mut connection: Connection,
    instance_id: String,
    composed: Arc<Composed>,
    discovering: Arc<Mutex<()>>,
    native_cursors: Arc<NativeCursorCodec>,
    sessions: watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    asking: mpsc::Sender<RuntimeAsked>,
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
        let catalogue = Arc::clone(&sessions.borrow());
        let answered = answer(
            &mut state,
            &instance_id,
            &composed,
            &discovering,
            &native_cursors,
            &catalogue,
            &asking,
            &returning,
            request,
        )
        .await;
        if send_response(&mut connection, &answered.response)
            .await
            .is_err()
            || answered.close
        {
            return;
        }
        if let Some(watching) = answered.watching {
            relay_events(&mut connection, watching).await;
            return;
        }
    }
}

enum PublicState {
    Fresh {
        challenge: runtrol_runtime_protocol::ServerChallenge,
    },
    Negotiated {
        context: ClientContext,
        authority: PublicAuthority,
    },
    Ready {
        context: ClientContext,
        authority: PublicAuthority,
    },
}

enum PublicAuthority {
    Anonymous,
    Pending(EnrollmentKey),
    Authorized(AuthorizedIntegration),
}

struct Answer {
    response: JsonRpcResponse,
    close: bool,
    watching: Option<Watching>,
}

impl Answer {
    fn success<T: Serialize>(id: JsonRpcId, result: &T) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: None,
        }
    }

    fn failure(id: JsonRpcId, failure: AuthorizationFailure) -> Self {
        let close = failure.kind == RuntimeErrorKind::IntegrationRevoked;
        Self {
            response: failure_response(id, failure.kind, failure.message),
            close,
            watching: None,
        }
    }

    fn plain(id: JsonRpcId, code: RuntimeErrorKind, message: &str) -> Self {
        Self {
            response: failure_response(id, code, message),
            close: false,
            watching: None,
        }
    }

    fn watching(
        id: JsonRpcId,
        result: &WatchEventsResult,
        view: Box<runtrol_core::SessionView>,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching {
                subscription_id: result.subscription_id.clone(),
                session_id: result.session_id.clone(),
                view,
            }),
        }
    }
}

struct Watching {
    subscription_id: String,
    session_id: RuntimeSessionId,
    view: Box<runtrol_core::SessionView>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one audited request keeps connection authority, discovery admission, session state, and owner channels explicit"
)]
async fn answer(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    discovering: &Mutex<()>,
    native_cursors: &NativeCursorCodec,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<RuntimeAsked>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    request: JsonRpcRequest,
) -> Answer {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        if crate::runtime_audit::structural(
            &composed.store,
            "runtime/invalidEnvelope",
            RuntimeErrorKind::InvalidRequest.as_str(),
        )
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
        if crate::runtime_audit::structural(
            &composed.store,
            "runtime/unknownMethod",
            RuntimeErrorKind::MethodNotFound.as_str(),
        )
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
    if crate::runtime_audit::public(
        &composed.store,
        integration,
        key_generation,
        method,
        scope,
        IntegrationAuditOutcome::Attempted,
        "attempted",
    )
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
        sessions,
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
    let (integration, key_generation) = audit_identity(state);
    if crate::runtime_audit::public(
        &composed.store,
        integration,
        key_generation,
        method,
        scope,
        outcome,
        reason,
    )
    .is_err()
    {
        return audit_unavailable(audit_response_id);
    }
    answered
}

#[expect(
    clippy::too_many_arguments,
    reason = "public dispatch keeps connection authority, immutable catalogues, owner channels, and the exact JSON-RPC request together"
)]
async fn dispatch_public(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    discovering: &Mutex<()>,
    native_cursors: &NativeCursorCodec,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<RuntimeAsked>,
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
            RuntimeMethod::Initialize => initialize(state, instance_id, composed, id, params),
            RuntimeMethod::IntegrationsRequestEnrollment => {
                request_integration(state, composed, id, params)
            }
            RuntimeMethod::IntegrationsWatchEnrollment => {
                watch_integration(state, composed, id, params)
            }
            RuntimeMethod::IntegrationsGetGrant => grant(state, composed, id, params),
            RuntimeMethod::ProvidersList => providers(state, composed, id, params),
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
            RuntimeMethod::SessionsList => sessions_list(state, composed, sessions, id, params),
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
            | RuntimeMethod::SessionsWatchEvents
            | RuntimeMethod::SessionsInterrupt => {
                session_operation(
                    state, composed, sessions, asking, returning, method, id, params,
                )
                .await
            }
            RuntimeMethod::Initialized
            | RuntimeMethod::Challenge
            | RuntimeMethod::SessionsEvent
            | RuntimeMethod::SessionsLagged => Answer::plain(
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

fn required_scope(method: RuntimeMethod) -> Option<AppScope> {
    match method {
        RuntimeMethod::ProvidersList | RuntimeMethod::ProvidersGetCapabilities => {
            Some(AppScope::ProviderRead)
        }
        RuntimeMethod::ProvidersListModels => Some(AppScope::ModelRead),
        RuntimeMethod::ProvidersListNativeSessions => Some(AppScope::SessionNativeDiscover),
        RuntimeMethod::SessionsList | RuntimeMethod::SessionsGet => Some(AppScope::SessionList),
        RuntimeMethod::SessionsStart => Some(AppScope::SessionStart),
        RuntimeMethod::SessionsAdoptNative | RuntimeMethod::SessionsResume => {
            Some(AppScope::SessionResume)
        }
        RuntimeMethod::SessionsAcquireControl | RuntimeMethod::SessionsSubmitInput => {
            Some(AppScope::SessionInputWrite)
        }
        RuntimeMethod::SessionsWatchEvents => Some(AppScope::SessionOutputRead),
        RuntimeMethod::SessionsInterrupt => Some(AppScope::SessionStop),
        RuntimeMethod::SessionsRenewControl
        | RuntimeMethod::SessionsReleaseControl
        | RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::PanicStop => None,
    }
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

fn initialize(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Fresh { challenge } = state else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "Runtime initialization cannot be repeated on one connection",
        );
    };
    let Ok(params) = serde_json::from_value::<InitializeParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "initialization parameters are invalid",
        );
    };
    if params.supported_revisions.is_empty()
        || params.supported_revisions.len() > usize::from(MAX_REVISION_OFFERS)
        || params
            .supported_revisions
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != params.supported_revisions.len()
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the finalized revision offer is empty, duplicated, or oversized",
        );
    }
    let Some(revision) = negotiate(&params.supported_revisions, &FINALIZED_REVISIONS) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::ProtocolIncompatible,
            "no finalized Runtime revision is shared",
        );
    };
    let context = ClientContext {
        challenge: challenge.clone(),
        supported_revisions: params.supported_revisions,
        selected_revision: revision,
        client: params.client,
        capabilities: params.client_capabilities,
    };
    let authority = match params.authentication.as_ref() {
        Some(proof) => match authenticate(&composed.store, &context, proof) {
            Ok(authorized) => PublicAuthority::Authorized(authorized),
            Err(failure) => return Answer::failure(id, failure),
        },
        None => PublicAuthority::Anonymous,
    };
    let granted = match &authority {
        PublicAuthority::Authorized(authorized) => Some(authorized.grant.clone()),
        PublicAuthority::Anonymous | PublicAuthority::Pending(_) => None,
    };
    let result = InitializeResult {
        selected_revision: revision,
        runtime: RuntimeInstance {
            instance_id: instance_id.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: platform_name().to_owned(),
        },
        server_capabilities: RuntimeCapabilities {
            integration_enrollment: true,
            provider_inventory: true,
            managed_session_list: true,
            model_discovery: true,
            native_session_catalogue: true,
            session_control: true,
            session_events: true,
        },
        limits: RuntimeLimits::default(),
        grant: granted,
    };
    *state = PublicState::Negotiated { context, authority };
    Answer::success(id, &result)
}

fn request_integration(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Ready { context, authority } = state else {
        return not_ready(id);
    };
    if !matches!(authority, PublicAuthority::Anonymous) {
        return Answer::plain(
            id,
            RuntimeErrorKind::EnrollmentPending,
            "this connection already has integration authority or a pending decision",
        );
    }
    let Ok(params) = serde_json::from_value::<RequestEnrollmentParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration enrollment parameters are invalid",
        );
    };
    match request_enrollment(&composed.store, context, &params) {
        Ok((pending, receipt)) => {
            *authority = PublicAuthority::Pending(pending);
            Answer::success(id, &receipt)
        }
        Err(failure) => Answer::failure(id, failure),
    }
}

fn watch_integration(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Ready { authority, .. } = state else {
        return not_ready(id);
    };
    let PublicAuthority::Pending(expected) = authority else {
        return Answer::plain(
            id,
            RuntimeErrorKind::Unauthenticated,
            "this connection has no proved pending enrollment",
        );
    };
    let Ok(params) = serde_json::from_value::<WatchEnrollmentParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "enrollment watch parameters are invalid",
        );
    };
    match enrollment_decision(&composed.store, *expected, &params.pending_id) {
        Ok(decision) => Answer::success(id, &decision),
        Err(failure) => Answer::failure(id, failure),
    }
}

fn grant(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration grant parameters are invalid",
        );
    }
    match authorized(state, &composed.store, None) {
        Ok(authority) => Answer::success(id, &authority.grant),
        Err(failure) => Answer::failure(id, failure),
    }
}

fn providers(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider list parameters are invalid",
        );
    }
    match authorized(state, &composed.store, Some(AppScope::ProviderRead)) {
        Ok(_) => Answer::success(id, &crate::runtime_inventory::providers(composed)),
        Err(failure) => Answer::failure(id, failure),
    }
}

async fn get_provider_capabilities(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &Mutex<()>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<GetProviderCapabilitiesParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider capability parameters are invalid",
        );
    };
    if let Err(failure) = authorized(state, &composed.store, Some(AppScope::ProviderRead)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider_id) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _preparing = discovering.lock().await;
            crate::provider_prepare::driver(composed, provider_id).await
        },
    )
    .await;
    let driver = match discovered {
        Ok(Ok(driver)) => driver,
        Ok(Err(_)) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider could not supply structural capabilities",
            );
        }
        Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::RuntimeUnavailable,
                "provider capability discovery exceeded its bounded deadline",
            );
        }
    };
    if let Err(failure) = authorized(state, &composed.store, Some(AppScope::ProviderRead)) {
        return Answer::failure(id, failure);
    }
    Answer::success(
        id,
        &provider_capabilities(params.provider_id, driver.capabilities()),
    )
}

fn provider_capabilities(
    provider_id: runtrol_runtime_protocol::ProviderId,
    capabilities: runtrol_provider::ProviderCapabilities,
) -> RuntimeProviderCapabilities {
    RuntimeProviderCapabilities {
        provider_id,
        freshness: runtrol_runtime_protocol::CapabilityFreshness::Current,
        fresh_session: provider_capability(capabilities.fresh_session),
        resume: provider_capability(capabilities.resume),
        structured_events: provider_capability(capabilities.structured_events),
        interrupt: provider_capability(capabilities.interrupt),
        approvals: provider_capability(capabilities.approvals),
        cooling: provider_capability(capabilities.cooling),
        native_session_catalogue: provider_capability(capabilities.native_session_catalogue),
    }
}

fn provider_capability(
    capability: runtrol_provider::ProviderCapability,
) -> ProviderCapabilityObservation {
    ProviderCapabilityObservation {
        availability: match capability.state {
            runtrol_provider::ProviderCapabilityState::Available => {
                ProviderCapabilityAvailability::Available
            }
            runtrol_provider::ProviderCapabilityState::Unsupported => {
                ProviderCapabilityAvailability::Unsupported
            }
            runtrol_provider::ProviderCapabilityState::Unknown => {
                ProviderCapabilityAvailability::Unknown
            }
        },
        provenance: capability.source.map(|source| match source {
            runtrol_provider::ProviderCapabilitySource::OfficialProtocol => {
                ProviderCapabilityProvenance::OfficialProtocol
            }
            runtrol_provider::ProviderCapabilitySource::OfficialCli => {
                ProviderCapabilityProvenance::OfficialCli
            }
            runtrol_provider::ProviderCapabilitySource::DriverContract => {
                ProviderCapabilityProvenance::DriverContract
            }
        }),
        why: capability.why.map(String::from),
    }
}

async fn list_models(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &Mutex<()>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<ListModelsParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "model catalogue parameters are invalid",
        );
    };
    if let Err(failure) = authorized(state, &composed.store, Some(AppScope::ModelRead)) {
        return Answer::failure(id, failure);
    }
    let Ok(provider_id) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _preparing = discovering.lock().await;
            let driver = crate::provider_prepare::driver(composed, provider_id)
                .await
                .map_err(|_| ())?;
            driver.models().await.map_err(|_| ())
        },
    )
    .await;
    match discovered {
        Ok(Ok(catalogue)) => Answer::success(id, &model_catalogue(catalogue)),
        Ok(Err(())) => Answer::plain(
            id,
            RuntimeErrorKind::ProviderUnavailable,
            "the selected provider could not supply a model catalogue",
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "model catalogue discovery exceeded its bounded deadline",
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "native discovery keeps authority, provider preparation, cursor binding, and managed-session merging explicit"
)]
async fn list_native_sessions(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &Mutex<()>,
    native_cursors: &NativeCursorCodec,
    managed: &RuntimeSessionCatalogue,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<ListNativeSessionsParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "native session catalogue parameters are invalid",
        );
    };
    if params
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.len() > MAX_NATIVE_PUBLIC_CURSOR_BYTES)
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native session catalogue cursor is oversized",
        );
    }
    let authority = match authorized(
        state,
        &composed.store,
        Some(AppScope::SessionNativeDiscover),
    ) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let selected_root = match crate::runtime_inventory::authorized_root(&authority, &params.root) {
        Ok(root) => root,
        Err(failure) => return inventory_failure(id, failure),
    };
    let approved_roots = match crate::runtime_inventory::authorized_roots(&authority) {
        Ok(roots) => roots,
        Err(failure) => return inventory_failure(id, failure),
    };
    let Ok(provider) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let discovered = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _preparing = discovering.lock().await;
            let prepared = crate::provider_prepare::prepared_driver(composed, provider)
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            let opened = params
                .cursor
                .as_deref()
                .map(|cursor| {
                    native_cursors.open(
                        &authority,
                        provider,
                        &selected_root,
                        prepared.binary_identity,
                        cursor,
                    )
                })
                .transpose()
                .map_err(NativeDiscoveryFailure::Cursor)?;
            let catalogue = prepared
                .driver
                .native_sessions(runtrol_provider::NativeSessionQuery {
                    root: selected_root.path.clone(),
                    cursor: opened.as_ref().map(|cursor| cursor.provider_cursor.clone()),
                    limit: MAX_PAGE_ITEMS,
                })
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            let next = catalogue.next_cursor.clone();
            let mut public = crate::runtime_native_sessions::authorize_catalogue(
                native_cursors,
                &authority,
                &selected_root,
                &approved_roots,
                managed,
                provider,
                prepared.binary_identity,
                catalogue,
            )
            .map_err(NativeDiscoveryFailure::Inventory)?;
            if let Some(next) = next {
                public.next_cursor = Some(
                    native_cursors
                        .seal(
                            &authority,
                            provider,
                            &selected_root,
                            prepared.binary_identity,
                            &next,
                            opened.as_ref(),
                        )
                        .map_err(NativeDiscoveryFailure::Cursor)?,
                );
            }
            Ok(public)
        },
    )
    .await;
    match discovered {
        Ok(Ok(catalogue)) => Answer::success(id, &catalogue),
        Ok(Err(NativeDiscoveryFailure::Cursor(failure))) => cursor_failure(id, failure),
        Ok(Err(NativeDiscoveryFailure::Inventory(failure))) => inventory_failure(id, failure),
        Ok(Err(NativeDiscoveryFailure::Provider)) => Answer::plain(
            id,
            RuntimeErrorKind::ProviderUnavailable,
            "the selected provider could not supply a native session catalogue",
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "native session discovery exceeded its bounded deadline",
        ),
    }
}

enum NativeDiscoveryFailure {
    Cursor(NativeCursorFailure),
    Inventory(RuntimeInventoryFailure),
    Provider,
}

fn cursor_failure(id: JsonRpcId, failure: NativeCursorFailure) -> Answer {
    match failure {
        NativeCursorFailure::Invalid | NativeCursorFailure::Expired => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the native session catalogue cursor is invalid, expired, or outside this context",
        ),
        NativeCursorFailure::TooManyPages => Answer::plain(
            id,
            RuntimeErrorKind::ResourceExhausted,
            "the native session catalogue exceeded its bounded page walk",
        ),
        NativeCursorFailure::Internal => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not protect the native session catalogue cursor",
        ),
    }
}

fn model_catalogue(catalogue: runtrol_provider::ModelCatalog) -> RuntimeModelCatalog {
    match catalogue {
        runtrol_provider::ModelCatalog::Known { models } => RuntimeModelCatalog::Known {
            models: models.into_iter().map(model_choice).collect(),
        },
        runtrol_provider::ModelCatalog::Aliases { aliases, why } => RuntimeModelCatalog::Aliases {
            aliases: aliases.into_iter().map(String::from).collect(),
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Partial {
            aliases,
            models,
            why,
        } => RuntimeModelCatalog::Partial {
            aliases: aliases.into_iter().map(String::from).collect(),
            models: models.into_iter().map(model_choice).collect(),
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Unknown { why } => RuntimeModelCatalog::Unknown {
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Unsupported { why } => RuntimeModelCatalog::Unsupported {
            why: String::from(why),
        },
        _ => RuntimeModelCatalog::Unknown {
            why: "the provider returned catalogue coverage unsupported by this Runtime".to_owned(),
        },
    }
}

fn model_choice(choice: runtrol_provider::ModelChoice) -> RuntimeModelChoice {
    RuntimeModelChoice {
        id: String::from(choice.id),
        display_name: String::from(choice.display_name),
        description: String::from(choice.description),
        is_default: choice.is_default,
        reasoning_efforts: choice
            .reasoning_efforts
            .into_iter()
            .map(|effort| RuntimeReasoningChoice {
                id: String::from(effort.id),
                description: String::from(effort.description),
            })
            .collect(),
    }
}

fn sessions_list(
    state: &mut PublicState,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "session list parameters are invalid",
        );
    }
    match authorized(state, &composed.store, Some(AppScope::SessionList)) {
        Ok(authority) => match sessions.authorized(authority) {
            Ok(list) => Answer::success(id, &list),
            Err(RuntimeInventoryFailure::Unavailable) => Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "the managed session catalogue is temporarily unavailable",
            ),
            Err(RuntimeInventoryFailure::RootAuthorityChanged) => Answer::plain(
                id,
                RuntimeErrorKind::RootDenied,
                "an approved project root no longer names the directory approved locally",
            ),
            Err(RuntimeInventoryFailure::SessionNotFound) => Answer::plain(
                id,
                RuntimeErrorKind::SessionNotFound,
                "the Runtime session does not exist in the integration grant",
            ),
        },
        Err(failure) => Answer::failure(id, failure),
    }
}

fn sessions_get(
    state: &mut PublicState,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<GetSessionParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "session descriptor parameters are invalid",
        );
    };
    match authorized(state, &composed.store, Some(AppScope::SessionList)) {
        Ok(authority) => match sessions.authorized_descriptor(authority, &params.session_id) {
            Ok(descriptor) => Answer::success(id, &descriptor),
            Err(failure) => inventory_failure(id, failure),
        },
        Err(failure) => Answer::failure(id, failure),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one public session boundary keeps authorization, catalogue resolution, owner handoff, and response identity visible together"
)]
async fn session_operation(
    state: &mut PublicState,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<RuntimeAsked>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let parsed = match parse_session_operation(method, params) {
        Ok(parsed) => parsed,
        Err(message) => return Answer::plain(id, RuntimeErrorKind::InvalidRequest, message),
    };
    let authority = match authorized(state, &composed.store, required_scope(method)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let session = match sessions.authorized_session(&authority, parsed.session_id()) {
        Ok(session) => session,
        Err(failure) => return inventory_failure(id, failure),
    };
    let request = parsed.into_owner_request(session);
    let (answered, hearing) = oneshot::channel();
    if asking
        .send(RuntimeAsked {
            integration: authority.key,
            request,
            answered,
        })
        .await
        .is_err()
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "the Runtime session owner stopped",
        );
    }
    let Ok(reply) = hearing.await else {
        return Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "the Runtime session owner stopped",
        );
    };
    runtime_control_answer(id, reply, returning).await
}

#[expect(
    clippy::too_many_arguments,
    reason = "the public open boundary keeps closed parsing, live authority, workspace identity, and owner reservation ordering together"
)]
async fn open_session(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &Mutex<()>,
    native_cursors: &NativeCursorCodec,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<RuntimeAsked>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let authority = match authorized(state, &composed.store, required_scope(method)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let request = match build_open_request(method, params, &authority, sessions) {
        Ok(request) => request,
        Err(OpenAdmissionFailure::Control(failure)) => return control_failure(id, failure),
        Err(OpenAdmissionFailure::Inventory(failure)) => return inventory_failure(id, failure),
    };
    let (answered, hearing) = oneshot::channel();
    if asking
        .send(RuntimeAsked {
            integration: authority.key,
            request: RuntimeControlRequest::PrepareOpen(request),
            answered,
        })
        .await
        .is_err()
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "the Runtime session owner stopped",
        );
    }
    let Ok(reply) = hearing.await else {
        return Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            "the Runtime session owner stopped",
        );
    };
    match reply {
        RuntimeControlReply::Opening(opening) => {
            perform_runtime_open(
                state,
                composed,
                discovering,
                native_cursors,
                returning,
                *opening,
                id,
            )
            .await
        }
        RuntimeControlReply::Opened(result) => Answer::success(id, &result),
        RuntimeControlReply::Failed(failure) => control_failure(id, failure),
        RuntimeControlReply::Lease(_)
        | RuntimeControlReply::Done
        | RuntimeControlReply::Watching { .. }
        | RuntimeControlReply::Sending { .. } => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "the session owner returned a mismatched open response",
        ),
    }
}

enum OpenAdmissionFailure {
    Control(RuntimeControlFailure),
    Inventory(RuntimeInventoryFailure),
}

#[expect(
    clippy::too_many_lines,
    reason = "closed start, adoption, and resume shapes share one explicit authority-to-owner admission boundary"
)]
fn build_open_request(
    method: RuntimeMethod,
    params: serde_json::Value,
    authority: &AuthorizedIntegration,
    sessions: &RuntimeSessionCatalogue,
) -> Result<RuntimeOpenRequest, OpenAdmissionFailure> {
    match method {
        RuntimeMethod::SessionsStart => {
            let params: StartSessionParams = serde_json::from_value(params).map_err(|_| {
                OpenAdmissionFailure::Control(invalid_open("session start parameters are invalid"))
            })?;
            let provider = parse_open_provider(&params.provider_id)?;
            let workspace = authorized_workspace(authority, &params.workspace)
                .map_err(OpenAdmissionFailure::Inventory)?;
            let access = public_open_access(params.access)?;
            let model = params.model.map(validate_model_selection).transpose()?;
            let claim = WorkspaceClaim::discover(workspace.path.clone(), access)
                .map_err(|_| OpenAdmissionFailure::Control(workspace_conflict()))?;
            Ok(RuntimeOpenRequest {
                method,
                request_id: params.request_id,
                provider,
                session: None,
                native: None,
                workspace: workspace.path,
                claim,
                model,
                expected: None,
                proof: None,
            })
        }
        RuntimeMethod::SessionsAdoptNative => {
            let params: AdoptNativeSessionParams =
                serde_json::from_value(params).map_err(|_| {
                    OpenAdmissionFailure::Control(invalid_open(
                        "native session adoption parameters are invalid",
                    ))
                })?;
            let provider = parse_open_provider(&params.provider_id)?;
            let native = runtrol_provider::NativeSessionId::new(&params.native_session_id)
                .map_err(|_| {
                    OpenAdmissionFailure::Control(invalid_open(
                        "native session identity is invalid",
                    ))
                })?;
            if params.adoption_token.len() > MAX_NATIVE_ADOPTION_TOKEN_BYTES {
                return Err(OpenAdmissionFailure::Control(invalid_open(
                    "native session adoption proof is oversized",
                )));
            }
            let workspace = authorized_workspace(authority, &params.workspace)
                .map_err(OpenAdmissionFailure::Inventory)?;
            let access = public_open_access(params.access)?;
            let claim = WorkspaceClaim::discover(workspace.path.clone(), access)
                .map_err(|_| OpenAdmissionFailure::Control(workspace_conflict()))?;
            Ok(RuntimeOpenRequest {
                method,
                request_id: params.request_id,
                provider,
                session: None,
                native: Some(native),
                workspace: workspace.path,
                claim,
                model: None,
                expected: None,
                proof: Some(params.adoption_token.into()),
            })
        }
        RuntimeMethod::SessionsResume => {
            let params: ResumeSessionParams = serde_json::from_value(params).map_err(|_| {
                OpenAdmissionFailure::Control(invalid_open("session resume parameters are invalid"))
            })?;
            let managed = sessions
                .authorized_managed_session(authority, &params.session_id)
                .map_err(OpenAdmissionFailure::Inventory)?;
            let workspace = authorized_workspace(authority, &params.workspace)
                .map_err(OpenAdmissionFailure::Inventory)?;
            if workspace.path != managed.workspace
                || params.expected_lifecycle != managed.descriptor.lifecycle
                || params.expected_session_generation != managed.descriptor.session_generation
            {
                return Err(OpenAdmissionFailure::Control(session_changed()));
            }
            let Some(native) = managed.native else {
                return Err(OpenAdmissionFailure::Control(RuntimeControlFailure::new(
                    RuntimeErrorKind::CapabilityUnavailable,
                    "the managed session has no provider-native resume identity",
                )));
            };
            let native = runtrol_provider::NativeSessionId::new(&native).map_err(|_| {
                OpenAdmissionFailure::Control(RuntimeControlFailure::new(
                    RuntimeErrorKind::CapabilityUnavailable,
                    "the managed session has no usable provider-native resume identity",
                ))
            })?;
            let access = public_open_access(params.access)?;
            let claim = WorkspaceClaim::discover(workspace.path.clone(), access)
                .map_err(|_| OpenAdmissionFailure::Control(workspace_conflict()))?;
            Ok(RuntimeOpenRequest {
                method,
                request_id: params.request_id,
                provider: managed.provider,
                session: Some(managed.session),
                native: Some(native),
                workspace: workspace.path,
                claim,
                model: None,
                expected: Some((
                    params.expected_lifecycle,
                    params.expected_session_generation,
                )),
                proof: None,
            })
        }
        _ => Err(OpenAdmissionFailure::Control(invalid_open(
            "the method is not a session open operation",
        ))),
    }
}

fn parse_open_provider(
    provider: &runtrol_runtime_protocol::ProviderId,
) -> Result<runtrol_provider::ProviderId, OpenAdmissionFailure> {
    runtrol_provider::ProviderId::parse(provider.as_str()).map_err(|_| {
        OpenAdmissionFailure::Control(invalid_open("the selected provider identity is invalid"))
    })
}

fn public_open_access(
    access: SessionWorkspaceAccess,
) -> Result<WorkspaceAccess, OpenAdmissionFailure> {
    match access {
        SessionWorkspaceAccess::Exclusive => Ok(WorkspaceAccess::Exclusive),
        SessionWorkspaceAccess::Shared => {
            Err(OpenAdmissionFailure::Control(RuntimeControlFailure::new(
                RuntimeErrorKind::PresenceRequired,
                "shared writer admission requires a local operator action",
            )))
        }
    }
}

fn validate_model_selection(value: String) -> Result<Box<str>, OpenAdmissionFailure> {
    if value.is_empty()
        || value.len() > MAX_MODEL_SELECTION_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(OpenAdmissionFailure::Control(invalid_open(
            "the model selection is empty, oversized, or invalid",
        )));
    }
    Ok(value.into())
}

#[expect(
    clippy::too_many_lines,
    reason = "one open future owns displaced cleanup, authority refresh, provider preparation, official selection checks, cancellation, and owner return"
)]
async fn perform_runtime_open(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &Mutex<()>,
    native_cursors: &NativeCursorCodec,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    opening: crate::runtime_control::RuntimeOpening,
    id: JsonRpcId,
) -> Answer {
    let mut guard = RuntimeOpenGuard::new(opening, returning.clone());
    if let Some(displaced) = guard.take_displaced_agent()
        && displaced
            .close(CloseMode::Graceful { grace_ms: 0 })
            .await
            .is_err()
    {
        return send_open_denied(
            id,
            guard,
            returning,
            RuntimeControlFailure::new(
                RuntimeErrorKind::RuntimeUnavailable,
                "the displaced idle provider process could not be stopped safely",
            ),
        )
        .await;
    }
    let method = match guard.opening() {
        Some(opening) => opening.method,
        None => return control_failure(id, RuntimeControlFailure::outcome_unknown()),
    };
    let authority = match authorized(state, &composed.store, required_scope(method)) {
        Ok(authority) => authority.clone(),
        Err(failure) => {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(failure.kind, failure.message),
            )
            .await;
        }
    };
    let workspace = match guard.opening() {
        Some(opening) => opening.workspace.clone(),
        None => return control_failure(id, RuntimeControlFailure::outcome_unknown()),
    };
    match authorized_workspace(&authority, workspace.as_str()) {
        Ok(current) if current.path == workspace => {}
        Ok(_) | Err(_) => {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::RootDenied,
                    "the session workspace no longer has its approved root authority",
                ),
            )
            .await;
        }
    }
    let provider = match guard.opening() {
        Some(opening) => opening.provider,
        None => return control_failure(id, RuntimeControlFailure::outcome_unknown()),
    };
    let prepared = {
        let _gate = discovering.lock().await;
        crate::provider_prepare::prepared_driver(composed, provider).await
    };
    let Ok(prepared) = prepared else {
        return send_open_denied(
            id,
            guard,
            returning,
            RuntimeControlFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider could not be prepared",
            ),
        )
        .await;
    };
    if method == RuntimeMethod::SessionsAdoptNative {
        let Ok(roots) = authorized_roots(&authority) else {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::RootDenied,
                    "an approved project root no longer has local authority",
                ),
            )
            .await;
        };
        let verified = guard.opening().is_some_and(|opening| {
            let (Some(native), Some(proof)) = (&opening.native, &opening.proof) else {
                return false;
            };
            native_cursors
                .open_adoption(
                    &authority,
                    &roots,
                    provider,
                    prepared.binary_identity,
                    native.as_str(),
                    &opening.workspace,
                    proof,
                )
                .is_ok()
        });
        if !verified {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::CapabilityUnavailable,
                    "the native catalogue observation expired or no longer matches the provider",
                ),
            )
            .await;
        }
    }
    if let Some(model) = guard.opening().and_then(|opening| opening.model.as_deref()) {
        let discovered = prepared.driver.models().await;
        if !discovered.is_ok_and(|catalogue| model_is_current(&catalogue, model)) {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::ModelUnavailable,
                    "the selected model is not present in the provider's current catalogue",
                ),
            )
            .await;
        }
    }
    let intent = match guard.opening() {
        Some(opening) => OpenIntent {
            session: opening.session,
            workspace: opening.workspace.clone(),
            disposition: if opening.method == RuntimeMethod::SessionsStart {
                Disposition::Fresh
            } else {
                let Some(native) = &opening.native else {
                    return send_open_denied(
                        id,
                        guard,
                        returning,
                        RuntimeControlFailure::new(
                            RuntimeErrorKind::CapabilityUnavailable,
                            "the session has no provider-native resume identity",
                        ),
                    )
                    .await;
                };
                Disposition::Resume {
                    native: native.as_str().into(),
                }
            },
            model: opening.model.clone(),
            permission: None,
        },
        None => return control_failure(id, RuntimeControlFailure::outcome_unknown()),
    };
    let opened = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        prepared.driver.open(intent.clone()),
    )
    .await;
    match opened {
        Ok(Ok(agent)) => {
            let still_authorized = match authorized(state, &composed.store, required_scope(method))
            {
                Ok(authority) => authorized_workspace(authority, workspace.as_str())
                    .is_ok_and(|current| current.path == workspace),
                Err(_) => false,
            };
            if !still_authorized {
                drop(agent.close(CloseMode::Kill).await);
                return send_open_unknown(id, guard, returning).await;
            }
            send_opened(id, guard, returning, intent, agent).await
        }
        Ok(Err(_)) | Err(_) => send_open_unknown(id, guard, returning).await,
    }
}

fn model_is_current(catalogue: &runtrol_provider::ModelCatalog, selected: &str) -> bool {
    match catalogue {
        runtrol_provider::ModelCatalog::Known { models } => {
            models.iter().any(|model| model.id.as_ref() == selected)
        }
        runtrol_provider::ModelCatalog::Aliases { aliases, .. } => {
            aliases.iter().any(|alias| alias.as_ref() == selected)
        }
        runtrol_provider::ModelCatalog::Partial {
            aliases, models, ..
        } => {
            aliases.iter().any(|alias| alias.as_ref() == selected)
                || models.iter().any(|model| model.id.as_ref() == selected)
        }
        _ => false,
    }
}

async fn send_open_denied(
    id: JsonRpcId,
    guard: RuntimeOpenGuard,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    failure: RuntimeControlFailure,
) -> Answer {
    let Some(opening) = guard.take() else {
        return control_failure(id, RuntimeControlFailure::outcome_unknown());
    };
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::OpenDenied {
            opening,
            failure,
            answered,
        })
        .is_err()
    {
        return runtime_owner_stopped(id);
    }
    match hearing.await {
        Ok(completion) => finish_open_completion(id, completion, returning).await,
        Err(_) => runtime_owner_stopped(id),
    }
}

async fn send_open_unknown(
    id: JsonRpcId,
    guard: RuntimeOpenGuard,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
) -> Answer {
    let Some(opening) = guard.take() else {
        return control_failure(id, RuntimeControlFailure::outcome_unknown());
    };
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::OpenUnknown { opening, answered })
        .is_err()
    {
        return runtime_owner_stopped(id);
    }
    match hearing.await {
        Ok(completion) => finish_open_completion(id, completion, returning).await,
        Err(_) => runtime_owner_stopped(id),
    }
}

async fn send_opened(
    id: JsonRpcId,
    guard: RuntimeOpenGuard,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    intent: OpenIntent,
    agent: Box<dyn runtrol_provider::Agent>,
) -> Answer {
    let Some(opening) = guard.take() else {
        drop(agent);
        return control_failure(id, RuntimeControlFailure::outcome_unknown());
    };
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::Opened {
            opening,
            intent,
            agent,
            answered,
        })
        .is_err()
    {
        return runtime_owner_stopped(id);
    }
    match hearing.await {
        Ok(completion) => finish_open_completion(id, completion, returning).await,
        Err(_) => runtime_owner_stopped(id),
    }
}

async fn finish_open_completion(
    id: JsonRpcId,
    completion: RuntimeOpenCompletion,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
) -> Answer {
    match completion {
        RuntimeOpenCompletion::Answer(Ok(result)) => Answer::success(id, &result),
        RuntimeOpenCompletion::Answer(Err(failure)) => control_failure(id, failure),
        RuntimeOpenCompletion::Cleanup { agent, reservation } => {
            drop(agent.close(CloseMode::Kill).await);
            let (answered, hearing) = oneshot::channel();
            if returning
                .send(RuntimeReturned::OpenCleaned {
                    reservation,
                    answered,
                })
                .is_err()
            {
                return runtime_owner_stopped(id);
            }
            match hearing.await {
                Ok(Ok(result)) => Answer::success(id, &result),
                Ok(Err(failure)) => control_failure(id, failure),
                Err(_) => runtime_owner_stopped(id),
            }
        }
    }
}

fn runtime_owner_stopped(id: JsonRpcId) -> Answer {
    Answer::plain(
        id,
        RuntimeErrorKind::RuntimeUnavailable,
        "the Runtime session owner stopped",
    )
}

const fn invalid_open(message: &'static str) -> RuntimeControlFailure {
    RuntimeControlFailure::new(RuntimeErrorKind::InvalidRequest, message)
}

const fn workspace_conflict() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::WorkspaceConflict,
        "the workspace identity could not be established safely",
    )
}

const fn session_changed() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::SessionConflict,
        "the session changed after the caller observed it",
    )
}

enum ParsedSessionOperation {
    Acquire(AcquireControlParams),
    Renew(ControlLeaseParams),
    Release(ControlLeaseParams),
    Submit(SubmitInputParams),
    Watch {
        params: WatchEventsParams,
        subscription_id: String,
    },
    Interrupt(ControlLeaseParams),
}

impl ParsedSessionOperation {
    fn session_id(&self) -> &RuntimeSessionId {
        match self {
            Self::Acquire(params) => &params.session_id,
            Self::Renew(params) | Self::Release(params) | Self::Interrupt(params) => {
                &params.session_id
            }
            Self::Submit(params) => &params.session_id,
            Self::Watch { params, .. } => &params.session_id,
        }
    }

    fn into_owner_request(self, session: runtrol_provider::SessionId) -> RuntimeControlRequest {
        match self {
            Self::Acquire(params) => RuntimeControlRequest::Acquire { session, params },
            Self::Renew(params) => RuntimeControlRequest::Renew { session, params },
            Self::Release(params) => RuntimeControlRequest::Release { session, params },
            Self::Submit(params) => RuntimeControlRequest::Submit { session, params },
            Self::Watch {
                params,
                subscription_id,
            } => RuntimeControlRequest::Watch {
                session,
                params,
                subscription_id,
            },
            Self::Interrupt(params) => RuntimeControlRequest::Interrupt { session, params },
        }
    }
}

fn parse_session_operation(
    method: RuntimeMethod,
    params: serde_json::Value,
) -> Result<ParsedSessionOperation, &'static str> {
    match method {
        RuntimeMethod::SessionsAcquireControl => serde_json::from_value(params)
            .map(ParsedSessionOperation::Acquire)
            .map_err(|_| "session control acquisition parameters are invalid"),
        RuntimeMethod::SessionsRenewControl => serde_json::from_value(params)
            .map(ParsedSessionOperation::Renew)
            .map_err(|_| "session control renewal parameters are invalid"),
        RuntimeMethod::SessionsReleaseControl => serde_json::from_value(params)
            .map(ParsedSessionOperation::Release)
            .map_err(|_| "session control release parameters are invalid"),
        RuntimeMethod::SessionsSubmitInput => serde_json::from_value(params)
            .map(ParsedSessionOperation::Submit)
            .map_err(|_| "session input parameters are invalid"),
        RuntimeMethod::SessionsWatchEvents => {
            let params = serde_json::from_value(params)
                .map_err(|_| "session event watch parameters are invalid")?;
            Ok(ParsedSessionOperation::Watch {
                params,
                subscription_id: random_subscription_id()
                    .map_err(|_| "Runtime could not allocate a subscription identity")?,
            })
        }
        RuntimeMethod::SessionsInterrupt => serde_json::from_value(params)
            .map(ParsedSessionOperation::Interrupt)
            .map_err(|_| "session interrupt parameters are invalid"),
        RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::ProvidersList
        | RuntimeMethod::ProvidersGetCapabilities
        | RuntimeMethod::ProvidersListModels
        | RuntimeMethod::ProvidersListNativeSessions
        | RuntimeMethod::SessionsList
        | RuntimeMethod::SessionsGet
        | RuntimeMethod::SessionsStart
        | RuntimeMethod::SessionsAdoptNative
        | RuntimeMethod::SessionsResume
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::PanicStop => Err("the method is not a session operation"),
    }
}

async fn runtime_control_answer(
    id: JsonRpcId,
    reply: RuntimeControlReply,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
) -> Answer {
    match reply {
        RuntimeControlReply::Lease(lease) => Answer::success(id, &lease),
        RuntimeControlReply::Done => Answer::success(id, &EmptyResult {}),
        RuntimeControlReply::Watching { result, view } => Answer::watching(id, &result, view),
        RuntimeControlReply::Failed(failure) => control_failure(id, failure),
        RuntimeControlReply::Sending {
            mutation,
            taken,
            command,
        } => match perform_runtime_command(mutation, taken, command, returning.clone()).await {
            Some(Ok(())) => Answer::success(id, &EmptyResult {}),
            Some(Err(failure)) => control_failure(id, failure),
            None => Answer::plain(
                id,
                RuntimeErrorKind::RuntimeUnavailable,
                "the Runtime session owner stopped",
            ),
        },
        RuntimeControlReply::Opened(result) => Answer::success(id, &result),
        RuntimeControlReply::Opening(_) => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "a session open reservation reached the ordinary control response path",
        ),
    }
}

#[expect(
    clippy::manual_ok_err,
    reason = "Result::ok is forbidden because owner channel loss must remain explicit"
)]
async fn perform_runtime_command(
    mutation: runtrol_store::IntegrationMutationKey,
    taken: runtrol_core::TakenAgent,
    command: runtrol_provider::AgentCommand,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) -> Option<Result<(), RuntimeControlFailure>> {
    let runtrol_core::TakenAgent {
        agent: handed_agent,
        lease,
    } = taken;
    let guard = RuntimeAgentGuard::new(mutation, lease, returning.clone());
    let mut agent = handed_agent;
    let outcome = agent.send(command).await;
    let lease = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::Finished {
            mutation,
            taken: runtrol_core::TakenAgent { agent, lease },
            outcome,
            answered,
        })
        .is_err()
    {
        return None;
    }
    match hearing.await {
        Ok(result) => Some(result),
        Err(_) => None,
    }
}

fn control_failure(id: JsonRpcId, failure: RuntimeControlFailure) -> Answer {
    Answer::plain(id, failure.kind, failure.message)
}

fn inventory_failure(id: JsonRpcId, failure: RuntimeInventoryFailure) -> Answer {
    match failure {
        RuntimeInventoryFailure::Unavailable => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "the managed session catalogue is temporarily unavailable",
        ),
        RuntimeInventoryFailure::RootAuthorityChanged => Answer::plain(
            id,
            RuntimeErrorKind::RootDenied,
            "an approved project root no longer names the directory approved locally",
        ),
        RuntimeInventoryFailure::SessionNotFound => Answer::plain(
            id,
            RuntimeErrorKind::SessionNotFound,
            "the Runtime session does not exist in the integration grant",
        ),
    }
}

fn random_subscription_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    let mut output = String::from("sub_");
    for byte in bytes {
        use core::fmt::Write as _;
        if write!(&mut output, "{byte:02x}").is_err() {
            return Ok(String::new());
        }
    }
    Ok(output)
}

async fn relay_events(connection: &mut Connection, mut watching: Watching) {
    while let Some(item) = watching.view.recv().await {
        match item {
            runtrol_core::WatchItem::Event(event) => {
                let positioned = event.event();
                let next = runtrol_runtime_protocol::EventCursor {
                    stream: watching.view.start().live_at.stream.to_string(),
                    epoch: positioned.epoch,
                    seq: positioned.seq.wrapping_add(1),
                };
                let Ok(wire) = event.wire() else {
                    return;
                };
                let Ok((prefix, suffix)) = event_notification_edges(
                    &watching.subscription_id,
                    &watching.session_id,
                    &next,
                ) else {
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
                    subscription_id: watching.subscription_id,
                    session_id: watching.session_id,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventNotificationParams<'a> {
    subscription_id: &'a str,
    session_id: &'a RuntimeSessionId,
    event_revision: ProtocolRevision,
    event: (),
    next_expected: &'a runtrol_runtime_protocol::EventCursor,
}

fn event_notification_edges(
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

fn authorized<'a>(
    state: &'a mut PublicState,
    store: &runtrol_store::Store,
    needed: Option<AppScope>,
) -> Result<&'a AuthorizedIntegration, AuthorizationFailure> {
    let PublicState::Ready { authority, .. } = state else {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::NotInitialized,
            message: "Runtime initialization is not complete",
        });
    };
    let PublicAuthority::Authorized(current) = authority else {
        return Err(AuthorizationFailure {
            kind: match authority {
                PublicAuthority::Pending(_) => RuntimeErrorKind::EnrollmentPending,
                PublicAuthority::Anonymous => RuntimeErrorKind::Unauthenticated,
                PublicAuthority::Authorized(_) => RuntimeErrorKind::Internal,
            },
            message: "local integration approval and authenticated reconnect are required",
        });
    };
    *current = refresh(store, current)?;
    if needed.is_some_and(|scope| !current.grant.scopes.contains(&scope)) {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::ScopeDenied,
            message: "the integration grant lacks the required app scope",
        });
    }
    Ok(current)
}

fn not_ready(id: JsonRpcId) -> Answer {
    Answer::plain(
        id,
        RuntimeErrorKind::NotInitialized,
        "Runtime initialization is not complete",
    )
}

fn success<T: Serialize>(id: JsonRpcId, result: &T) -> JsonRpcResponse {
    match serde_json::to_value(result) {
        Ok(result) => JsonRpcResponse::Success(SuccessResponse {
            jsonrpc: "2.0".to_owned(),
            id,
            result,
        }),
        Err(_) => failure_response(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not encode its public result",
        ),
    }
}

fn failure_response(id: JsonRpcId, code: RuntimeErrorKind, message: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error(ErrorResponse {
        jsonrpc: "2.0".to_owned(),
        id,
        error: RuntimeError::plain(code, message, "runtime-public"),
    })
}

async fn send_response(
    connection: &mut Connection,
    response: &JsonRpcResponse,
) -> Result<(), runtrol_ipc::transport::TransportError> {
    let encoded = serde_json::to_vec(response).map_err(|error| {
        runtrol_ipc::transport::TransportError::Io {
            doing: "encoding a public Runtime response",
            detail: error.to_string(),
        }
    })?;
    connection.send(&encoded).await
}

async fn send_notification<T: Serialize>(
    connection: &mut Connection,
    method: RuntimeMethod,
    params: &T,
) -> Result<(), runtrol_ipc::transport::TransportError> {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: method.to_string(),
        params: serde_json::to_value(params).map_err(|error| {
            runtrol_ipc::transport::TransportError::Io {
                doing: "encoding public Runtime notification parameters",
                detail: error.to_string(),
            }
        })?,
    };
    let encoded = serde_json::to_vec(&notification).map_err(|error| {
        runtrol_ipc::transport::TransportError::Io {
            doing: "encoding a public Runtime notification",
            detail: error.to_string(),
        }
    })?;
    connection.send(&encoded).await
}

#[derive(serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Serialize)]
struct EmptyResult {}

#[cfg(windows)]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "windows-x86_64"
    } else {
        "windows-aarch64"
    }
}

#[cfg(target_os = "macos")]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "macos-x86_64"
    } else {
        "macos-aarch64"
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "linux-x86_64"
    } else {
        "linux-aarch64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64ct::Encoding as _;

    const NATIVE_PROVIDER_MANIFEST: &str = r#"
schema = 1
id = "native-fixture"
display_name = "Native Fixture"
kind = "native-fixture-kind"

[bin]
names = ["rustc"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
"#;

    fn make_native_fixture(
        context: &runtrol_drivers::DriverContext,
    ) -> Box<dyn runtrol_provider::Provider> {
        Box::new(NativeFixtureProvider {
            provider: context.provider,
        })
    }

    const NATIVE_PROVIDER_KINDS: &[runtrol_drivers::DriverKind] = &[runtrol_drivers::DriverKind {
        kind: "native-fixture-kind",
        make: Some(make_native_fixture),
        flags: &[],
        consult: runtrol_drivers::ConsultSurface {
            registrar: None,
            server: None,
        },
        unavailable: None,
    }];
    const NATIVE_PROVIDER_MANIFESTS: &[&str] = &[NATIVE_PROVIDER_MANIFEST];

    struct NativeFixtureProvider {
        provider: runtrol_provider::ProviderId,
    }

    struct NativeFixtureAgent {
        session: runtrol_provider::SessionId,
        native: String,
    }

    #[async_trait::async_trait]
    impl runtrol_provider::Agent for NativeFixtureAgent {
        fn session(&self) -> runtrol_provider::SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            Some(&self.native)
        }

        async fn send(
            &mut self,
            _command: runtrol_provider::AgentCommand,
        ) -> Result<(), runtrol_provider::ProviderError> {
            Ok(())
        }

        async fn next(
            &mut self,
        ) -> Option<Result<runtrol_provider::Produced, runtrol_provider::ProviderError>> {
            core::future::pending().await
        }

        async fn close(
            self: Box<Self>,
            _how: runtrol_provider::CloseMode,
        ) -> Result<(), runtrol_provider::ProviderError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl runtrol_provider::Provider for NativeFixtureProvider {
        fn id(&self) -> runtrol_provider::ProviderId {
            self.provider
        }

        async fn native_sessions(
            &self,
            query: runtrol_provider::NativeSessionQuery,
        ) -> Result<runtrol_provider::NativeSessionCatalogue, runtrol_provider::ProviderError>
        {
            let (native, next_cursor) = match query.cursor.as_deref() {
                None => ("fixture-native-one", Some("fixture-page-two".into())),
                Some("fixture-page-two") => ("fixture-native-two", None),
                Some(_) => {
                    return Err(runtrol_provider::ProviderError::Protocol {
                        provider: self.provider,
                        doing: "listing fixture sessions",
                        detail: "the cursor is unknown".to_owned(),
                    });
                }
            };
            Ok(runtrol_provider::NativeSessionCatalogue {
                coverage: runtrol_provider::NativeCatalogueCoverage::Complete {
                    source: runtrol_provider::NativeCatalogueSource::OfficialProtocol,
                },
                sessions: vec![runtrol_provider::NativeSessionEntry {
                    native: runtrol_provider::NativeSessionId::new(native)
                        .expect("valid fixture native identity"),
                    cwd: query.root.as_str().into(),
                    additional_directories: Vec::new(),
                    title: Some("Provider-owned fixture title".into()),
                    updated_at: Some("2026-08-13T00:00:00Z".into()),
                    resume: runtrol_provider::NativeResumeCapability::Available,
                }],
                next_cursor,
            })
        }

        async fn open(
            &self,
            intent: runtrol_provider::OpenIntent,
        ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
            let native = match &intent.disposition {
                runtrol_provider::Disposition::Fresh => intent.session.to_string(),
                runtrol_provider::Disposition::Resume { native } => native.to_string(),
                other => {
                    return Err(runtrol_provider::ProviderError::Unsupported {
                        provider: self.provider,
                        what: format!("opening with {other:?}"),
                        why: "the fixture supports only fresh and resumed sessions",
                    });
                }
            };
            Ok(Box::new(NativeFixtureAgent {
                session: intent.session,
                native,
            }))
        }
    }

    #[test]
    fn public_event_wrapper_preserves_the_existing_event_bytes() {
        let session = RuntimeSessionId::new("session_fixture");
        let next = runtrol_runtime_protocol::EventCursor {
            stream: "019c0000-0000-7000-8000-000000000001".to_owned(),
            epoch: 2,
            seq: 9,
        };
        let event = br#"{"body":{"text":"exact caller and provider bytes"}}"#;
        let (prefix, suffix) =
            event_notification_edges("sub_fixture", &session, &next).expect("event edges");
        let mut frame = prefix;
        frame.extend_from_slice(event);
        frame.extend_from_slice(&suffix);
        let notification: JsonRpcNotification =
            serde_json::from_slice(&frame).expect("valid notification");
        assert_eq!(notification.method, RuntimeMethod::SessionsEvent.as_str());
        assert_eq!(
            notification.params.get("event"),
            Some(&serde_json::json!({
                "body": {"text": "exact caller and provider bytes"}
            }))
        );
        assert!(frame.windows(event.len()).any(|window| window == event));
    }

    #[test]
    fn private_control_names_never_enter_the_public_method_table() {
        for private in [
            "hello",
            "list",
            "start",
            "providerUpdate",
            "private/control",
        ] {
            assert!(
                private.parse::<RuntimeMethod>().is_err(),
                "admitted {private:?}"
            );
        }
    }

    #[test]
    fn provider_model_catalogue_preserves_opaque_choices_and_coverage() {
        let catalogue = model_catalogue(runtrol_provider::ModelCatalog::Partial {
            aliases: vec!["provider-alias".into()],
            models: vec![runtrol_provider::ModelChoice {
                id: "provider-model".into(),
                display_name: "Provider Model".into(),
                description: "Provider description".into(),
                is_default: true,
                reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                    id: "provider-effort".into(),
                    description: "Provider effort".into(),
                }],
            }],
            why: "the provider reports a partial list".into(),
        });
        let RuntimeModelCatalog::Partial {
            aliases,
            models,
            why,
        } = catalogue
        else {
            panic!("coverage must remain partial");
        };
        assert_eq!(aliases, ["provider-alias"]);
        let model = models.first().expect("one mapped model");
        assert_eq!(model.id, "provider-model");
        assert_eq!(
            model
                .reasoning_efforts
                .first()
                .expect("one mapped reasoning effort")
                .id,
            "provider-effort"
        );
        assert_eq!(why, "the provider reports a partial list");

        assert!(matches!(
            model_catalogue(runtrol_provider::ModelCatalog::unsupported(
                "no official discovery surface"
            )),
            RuntimeModelCatalog::Unsupported { why }
                if why == "no official discovery surface"
        ));
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one real endpoint journey proves anonymous refusal, enrollment, approval, authenticated reconnect, inventory, and live revocation in sequence"
    )]
    async fn real_owner_only_runtime_initializes_but_reveals_nothing_before_enrollment() {
        let directory =
            std::env::temp_dir().join(format!("runtrol-runtime-public-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&directory));
        std::fs::create_dir_all(&directory).expect("create Runtime test directory");
        let project_path = directory.join("project");
        std::fs::create_dir(&project_path).expect("create approved project");
        let project = runtrol_provider::AbsPath::canonicalize(
            project_path.to_str().expect("UTF-8 approved project"),
        )
        .expect("canonical approved project");
        let project_identity = runtrol_security::ProjectRootIdentity::read(&project)
            .expect("read approved project identity")
            .to_bytes();
        let start_project_path = directory.join("start-project");
        std::fs::create_dir(&start_project_path).expect("create session start project");
        let start_project = runtrol_provider::AbsPath::canonicalize(
            start_project_path
                .to_str()
                .expect("UTF-8 session start project"),
        )
        .expect("canonical session start project");
        let start_project_identity = runtrol_security::ProjectRootIdentity::read(&start_project)
            .expect("read session start project identity")
            .to_bytes();
        let resume_project_path = directory.join("resume-project");
        std::fs::create_dir(&resume_project_path).expect("create session resume project");
        let resume_project = runtrol_provider::AbsPath::canonicalize(
            resume_project_path
                .to_str()
                .expect("UTF-8 session resume project"),
        )
        .expect("canonical session resume project");
        let resume_project_identity = runtrol_security::ProjectRootIdentity::read(&resume_project)
            .expect("read session resume project identity")
            .to_bytes();
        let endpoint = if cfg!(windows) {
            format!(r"\\.\pipe\runtrol-runtime-public-{}", std::process::id())
        } else {
            directory
                .join("runtrol-runtime.sock")
                .to_string_lossy()
                .into_owned()
        };
        let locator_path = directory.join("runtime.locator.json");
        let locator_abs = runtrol_provider::AbsPath::new(
            locator_path.to_str().expect("UTF-8 Runtime test locator"),
        )
        .expect("absolute Runtime test locator");
        let instance = "rtm_0123456789abcdef0123456789abcdef";
        let mut listener = runtrol_ipc::transport::Listener::bind_owner_only(&endpoint)
            .await
            .expect("bind owner-only Runtime endpoint");
        let published =
            crate::runtime_locator::PublishedLocator::publish(&locator_abs, instance, &endpoint)
                .expect("publish owner-only locator");
        let composed = Arc::new(
            crate::Composed::for_tests(
                directory.to_str().expect("UTF-8 Runtime test home"),
                runtrol_drivers::Builtin {
                    manifests: NATIVE_PROVIDER_MANIFESTS,
                    kinds: NATIVE_PROVIDER_KINDS,
                },
            )
            .expect("compose test Runtime"),
        );
        let fixture_provider =
            runtrol_provider::ProviderId::parse("native-fixture").expect("valid provider");
        let sessions = Arc::new(
            crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
                fixture_provider,
                "fixture-native-one",
                &resume_project,
            ),
        );
        let (_publishing, watching) = watch::channel(sessions);
        let (runtime_asking, runtime_asked) = mpsc::channel(1);
        let (runtime_returning, runtime_returned) = mpsc::unbounded_channel();
        let owning = tokio::spawn(crate::runtime_control::fixture_runtime_owner(
            Arc::clone(&composed),
            runtime_asked,
            runtime_returned,
        ));
        let discovering = Arc::new(Mutex::new(()));
        let native_cursors =
            Arc::new(NativeCursorCodec::new().expect("create native catalogue cursor authority"));
        let serving = tokio::spawn({
            let composed = Arc::clone(&composed);
            let discovering = Arc::clone(&discovering);
            let native_cursors = Arc::clone(&native_cursors);
            async move {
                for _ in 0..2 {
                    let connection = listener.accept().await.expect("accept public client");
                    serve_connection(
                        connection,
                        instance.to_owned(),
                        Arc::clone(&composed),
                        Arc::clone(&discovering),
                        Arc::clone(&native_cursors),
                        watching.clone(),
                        runtime_asking.clone(),
                        runtime_returning.clone(),
                    )
                    .await;
                }
            }
        });

        let locator = runtrol_runtime_client::RuntimeLocator::for_testing(&locator_path);
        let identity = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([7; 32]);
        let mut client = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_identity(identity.clone()),
            )
            .await
            .expect("initialize public client");
        let refused = client
            .providers()
            .list()
            .await
            .expect_err("inventory requires enrollment");
        assert!(matches!(
            refused,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::Unauthenticated
        ));

        let receipt = client
            .integrations()
            .request(runtrol_runtime_client::EnrollmentProposal::new(
                "fixture-instance",
                [3; 32],
                vec![
                    AppScope::ProviderRead,
                    AppScope::ModelRead,
                    AppScope::SessionList,
                    AppScope::SessionNativeDiscover,
                    AppScope::SessionStart,
                    AppScope::SessionResume,
                ],
                vec![
                    project.to_string(),
                    start_project.to_string(),
                    resume_project.to_string(),
                ],
            ))
            .await
            .expect("request enrollment");
        let pending = crate::runtime_auth::enrollment_key(&receipt.pending_id)
            .expect("valid pending identity");
        let public_key =
            match base64ct::Base64UrlUnpadded::decode_vec(&identity.public_key_base64()) {
                Ok(bytes) => <[u8; 32]>::try_from(bytes).expect("32-byte public key"),
                Err(error) => panic!("identity public key must decode: {error}"),
            };
        let integration = runtrol_store::IntegrationKey::from_bytes([9; 16]);
        composed
            .store
            .approve_enrollment(
                pending,
                integration,
                &runtrol_store::IntegrationRow {
                    public_key,
                    client_instance_id: "fixture-instance".into(),
                    label: "contract fixture".into(),
                    manifest_digest: [3; 32],
                    scopes: vec![
                        AppScope::ProviderRead.as_str().into(),
                        AppScope::ModelRead.as_str().into(),
                        AppScope::SessionList.as_str().into(),
                        AppScope::SessionNativeDiscover.as_str().into(),
                        AppScope::SessionStart.as_str().into(),
                        AppScope::SessionResume.as_str().into(),
                    ],
                    roots: vec![
                        runtrol_store::IntegrationRootRow {
                            path: project.as_str().into(),
                            identity: project_identity,
                        },
                        runtrol_store::IntegrationRootRow {
                            path: start_project.as_str().into(),
                            identity: start_project_identity,
                        },
                        runtrol_store::IntegrationRootRow {
                            path: resume_project.as_str().into(),
                            identity: resume_project_identity,
                        },
                    ],
                    key_generation: 1,
                    grant_generation: 1,
                    approved_at: runtrol_provider::WallMs::now(),
                    revoked_at: None,
                },
            )
            .expect("approve exact enrollment");
        let decision = client
            .integrations()
            .watch(receipt.pending_id)
            .await
            .expect("watch approved enrollment");
        let runtrol_runtime_protocol::EnrollmentDecision::Approved { grant } = decision else {
            panic!("the exact enrollment should be approved");
        };
        let credentials = client
            .credentials(grant.clone())
            .expect("bind returned grant to identity");

        drop(client);
        let mut approved = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_credentials(credentials),
            )
            .await
            .expect("authenticate approved client");
        assert_eq!(
            approved
                .integrations()
                .grant()
                .await
                .expect("current grant"),
            grant
        );
        approved
            .providers()
            .list()
            .await
            .expect("approved provider inventory");
        let capabilities = approved
            .providers()
            .get_capabilities(runtrol_runtime_protocol::ProviderId::new("native-fixture"))
            .await
            .expect("approved provider capability discovery");
        assert_eq!(
            capabilities.fresh_session.availability,
            runtrol_runtime_protocol::ProviderCapabilityAvailability::Unknown
        );
        assert_eq!(
            capabilities.freshness,
            runtrol_runtime_protocol::CapabilityFreshness::Current
        );
        let first = approved
            .providers()
            .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                root: project.to_string(),
                cursor: None,
            })
            .await
            .expect("first native catalogue page");
        assert_eq!(first.sessions.len(), 1);
        assert!(
            first
                .sessions
                .first()
                .is_some_and(|session| session.already_managed_as.is_some())
        );
        let managed_session = first
            .sessions
            .first()
            .and_then(|session| session.already_managed_as.clone())
            .expect("managed native fixture identity");
        let stored_session = managed_session
            .as_str()
            .parse::<runtrol_provider::SessionId>()
            .expect("managed Runtime session identity");
        let descriptor = approved
            .sessions()
            .get(managed_session.clone())
            .await
            .expect("read one exact managed session");
        assert_eq!(descriptor.session_id, managed_session);
        assert_eq!(
            descriptor.lifecycle,
            runtrol_runtime_protocol::LifecycleState::Cold
        );
        let now = runtrol_provider::WallMs::now();
        composed
            .store
            .put_session(
                stored_session,
                &runtrol_store::SessionRow {
                    provider: fixture_provider,
                    native: runtrol_provider::NativeSessionId::new("fixture-native-one")
                        .expect("fixture native identity"),
                    cwd: resume_project.clone(),
                    label: None,
                    created_at: now,
                    last_seen_at: now,
                    pinned: false,
                    archived: false,
                    forked_from: None,
                    live: None,
                },
            )
            .expect("store managed resume pointer");
        let resume_params = runtrol_runtime_protocol::ResumeSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            session_id: managed_session.clone(),
            expected_lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
            expected_session_generation: 0,
            workspace: resume_project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
        };
        let resumed = approved
            .sessions()
            .resume(&resume_params)
            .await
            .expect("resume an exact managed cold session");
        assert_eq!(resumed.session.session_id, managed_session);
        assert_eq!(
            approved
                .sessions()
                .resume(&resume_params)
                .await
                .expect("replay exact session resume"),
            resumed
        );
        let stale_resume = approved
            .sessions()
            .resume(&runtrol_runtime_protocol::ResumeSessionParams {
                request_id: runtrol_runtime_protocol::MutationRequestId::now(),
                session_id: managed_session,
                expected_lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
                expected_session_generation: 1,
                workspace: resume_project.to_string(),
                access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
            })
            .await
            .expect_err("stale resume generation is rejected");
        assert!(matches!(
            stale_resume,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::SessionConflict
        ));
        let denied_root = approved
            .providers()
            .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                root: directory.to_string_lossy().into_owned(),
                cursor: None,
            })
            .await
            .expect_err("an unapproved root cannot reach provider discovery");
        assert!(matches!(
            denied_root,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::RootDenied
        ));
        let mut tampered = first
            .next_cursor
            .clone()
            .expect("first page carries a cursor");
        tampered.push('x');
        let denied_cursor = approved
            .providers()
            .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                root: project.to_string(),
                cursor: Some(tampered),
            })
            .await
            .expect_err("a modified cursor is rejected before provider discovery");
        assert!(matches!(
            denied_cursor,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::InvalidRequest
        ));
        let second = approved
            .providers()
            .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                root: project.to_string(),
                cursor: first.next_cursor,
            })
            .await
            .expect("second native catalogue page");
        assert_eq!(second.sessions.len(), 1);
        assert!(second.next_cursor.is_none());
        let native = second.sessions.first().expect("second native session");
        let adoption_token = native
            .adoption_token
            .clone()
            .expect("unmanaged resumable session has an adoption proof");

        let start_params = runtrol_runtime_protocol::StartSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            workspace: start_project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
            model: None,
        };
        let started = approved
            .sessions()
            .start(&start_params)
            .await
            .expect("start an authorized fresh session");
        let repeated = approved
            .sessions()
            .start(&start_params)
            .await
            .expect("replay the exact session start");
        assert_eq!(repeated, started);
        let mut changed_start = start_params.clone();
        changed_start.model = Some("changed-model".to_owned());
        let conflict = approved
            .sessions()
            .start(&changed_start)
            .await
            .expect_err("changed start parameters cannot reuse a mutation identity");
        assert!(matches!(
            conflict,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::IdempotencyConflict
        ));
        let shared = approved
            .sessions()
            .start(&runtrol_runtime_protocol::StartSessionParams {
                request_id: runtrol_runtime_protocol::MutationRequestId::now(),
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                workspace: start_project.to_string(),
                access: runtrol_runtime_protocol::SessionWorkspaceAccess::Shared,
                model: None,
            })
            .await
            .expect_err("public shared writer admission requires local presence");
        assert!(matches!(
            shared,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::PresenceRequired
        ));

        let mut invalid_token = adoption_token.clone();
        invalid_token.push('x');
        let invalid_adoption = approved
            .sessions()
            .adopt_native(&runtrol_runtime_protocol::AdoptNativeSessionParams {
                request_id: runtrol_runtime_protocol::MutationRequestId::now(),
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                native_session_id: native.native_session_id.clone(),
                workspace: project.to_string(),
                access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
                adoption_token: invalid_token,
            })
            .await
            .expect_err("modified adoption proof is rejected");
        assert!(matches!(
            invalid_adoption,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::CapabilityUnavailable
        ));
        let adopted = approved
            .sessions()
            .adopt_native(&runtrol_runtime_protocol::AdoptNativeSessionParams {
                request_id: runtrol_runtime_protocol::MutationRequestId::now(),
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                native_session_id: native.native_session_id.clone(),
                workspace: project.to_string(),
                access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
                adoption_token,
            })
            .await
            .expect("adopt an exact native catalogue observation");
        assert_eq!(adopted.session.provider_id.as_str(), "native-fixture");
        let unavailable = approved
            .providers()
            .list_models(runtrol_runtime_protocol::ProviderId::new("not-registered"))
            .await
            .expect_err("an unknown provider cannot supply a model catalogue");
        assert!(matches!(
            unavailable,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::ProviderUnavailable
        ));
        assert!(
            composed
                .store
                .revoke_integration(integration, runtrol_provider::WallMs::now())
                .expect("revoke integration")
        );
        let revoked = approved
            .providers()
            .list()
            .await
            .expect_err("live revoked client is retired");
        assert!(matches!(
            revoked,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::IntegrationRevoked
        ));
        drop(approved);
        serving.await.expect("public server task finishes");
        owning.await.expect("Runtime owner task finishes");
        drop(published);
        drop(composed);
        drop(std::fs::remove_dir_all(directory));
    }
}
