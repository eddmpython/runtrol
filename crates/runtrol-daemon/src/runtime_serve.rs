//! Separate public Runtime listener with challenge-bound integration authority.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use runtrol_core::{ApprovalAuthority, WorkspaceClaim};
use runtrol_ipc::transport::Connection;
use runtrol_provider::{CloseMode, Disposition, OpenIntent, WorkspaceAccess};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, ArchiveNativeSessionParams,
    ControlLeaseParams, CoolSessionParams, DeleteNativeSessionParams, ErrorResponse,
    FINALIZED_REVISIONS, ForgetSessionParams, GetProviderCapabilitiesParams, GetSessionParams,
    InitializeParams, InitializeResult, JsonRpcId, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LaggedNotification, ListModelsParams, ListNativeSessionsParams,
    ListPendingApprovalsParams, MAX_MODEL_SELECTION_BYTES, MAX_NATIVE_ADOPTION_TOKEN_BYTES,
    MAX_NATIVE_PUBLIC_CURSOR_BYTES, MAX_PAGE_ITEMS, MAX_REVISION_OFFERS, ProtocolRevision,
    ProviderCapabilityAvailability, ProviderCapabilityObservation, ProviderCapabilityProvenance,
    ProviderList, ProviderUsageList, ProviderWatchEndReason, ProviderWatchEndedNotification,
    ProvidersChangedNotification, ProvidersUsageChangedNotification, RequestEnrollmentParams,
    RespondApprovalParams, ResumeSessionParams, RotateIntegrationKeyParams, RuntimeCapabilities,
    RuntimeError, RuntimeErrorKind, RuntimeInstance, RuntimeLimits, RuntimeMethod,
    RuntimeModelCatalog, RuntimeModelChoice, RuntimeProviderCapabilities, RuntimeReasoningChoice,
    RuntimeSessionId, SessionIndexChangedNotification, SessionIndexEndReason,
    SessionIndexEndedNotification, SessionWorkspaceAccess, SetModeParams, SetModelParams,
    StartSessionParams, SubmitBlocksParams, SubmitInputParams, SuccessResponse,
    WatchEnrollmentParams, WatchEventsParams, WatchEventsResult, WatchProvidersParams,
    WatchProvidersResult, WatchSessionIndexParams, WatchSessionIndexResult, negotiate,
};
use runtrol_store::IntegrationAuditOutcome;
use runtrol_store::{EnrollmentKey, IntegrationKeyRotation};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};

use crate::Composed;
use crate::runtime_auth::{
    AuthorizationFailure, AuthorizedIntegration, ClientContext, authenticate, challenge,
    enrollment_decision, refresh, request_enrollment,
};
use crate::runtime_control::{
    ApprovalScopes, RuntimeAgentGuard, RuntimeAsked, RuntimeControlFailure, RuntimeControlReply,
    RuntimeControlRequest, RuntimeCoolGuard, RuntimeCooling, RuntimeOpenCompletion,
    RuntimeOpenGuard, RuntimeOpenRequest, RuntimeReturned, cursor_to_public,
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
            relay_watch(&mut connection, watching, &mut sessions, composed.as_ref()).await;
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

    fn success_and_close<T: Serialize>(id: JsonRpcId, result: &T) -> Self {
        Self {
            response: success(id, result),
            close: true,
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

    fn operator_action(
        id: JsonRpcId,
        code: RuntimeErrorKind,
        message: &str,
        action: &str,
        correlation_id: String,
    ) -> Self {
        Self {
            response: JsonRpcResponse::Error(ErrorResponse {
                jsonrpc: "2.0".to_owned(),
                id,
                error: RuntimeError {
                    code,
                    message: message.to_owned(),
                    retryable: true,
                    operator_action: Some(action.to_owned()),
                    correlation_id,
                },
            }),
            close: false,
            watching: None,
        }
    }

    fn watching_events(
        id: JsonRpcId,
        result: &WatchEventsResult,
        view: Box<runtrol_core::SessionView>,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::Events {
                subscription_id: result.subscription_id.clone(),
                session_id: result.session_id.clone(),
                view,
            }),
        }
    }

    fn watching_index(
        id: JsonRpcId,
        result: &WatchSessionIndexResult,
        authority: AuthorizedIntegration,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::SessionIndex {
                subscription_id: result.subscription_id.clone(),
                last: result.snapshot.clone(),
                authority,
            }),
        }
    }

    fn watching_providers(
        id: JsonRpcId,
        result: &WatchProvidersResult,
        updates: watch::Receiver<Arc<ProviderList>>,
        usage: watch::Receiver<Arc<ProviderUsageList>>,
        authority: AuthorizedIntegration,
    ) -> Self {
        Self {
            response: success(id, result),
            close: false,
            watching: Some(Watching::Providers {
                subscription_id: result.subscription_id.clone(),
                last: result.snapshot.clone(),
                updates,
                usage,
                authority,
            }),
        }
    }
}

enum Watching {
    Events {
        subscription_id: String,
        session_id: RuntimeSessionId,
        view: Box<runtrol_core::SessionView>,
    },
    SessionIndex {
        subscription_id: String,
        last: runtrol_runtime_protocol::ManagedSessionList,
        authority: AuthorizedIntegration,
    },
    Providers {
        subscription_id: String,
        last: ProviderList,
        updates: watch::Receiver<Arc<ProviderList>>,
        usage: watch::Receiver<Arc<ProviderUsageList>>,
        authority: AuthorizedIntegration,
    },
}

#[expect(
    clippy::too_many_arguments,
    reason = "one audited request keeps connection authority, discovery admission, session state, and owner channels explicit"
)]
async fn answer(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
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
    clippy::too_many_lines,
    reason = "public dispatch keeps connection authority, immutable catalogues, owner channels, and the exact JSON-RPC request together"
)]
async fn dispatch_public(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &NativeCursorCodec,
    provider_updates: &watch::Sender<Arc<ProviderList>>,
    providers: &ProviderList,
    sessions: &RuntimeSessionCatalogue,
    usage: &ProviderUsageList,
    usage_updates: &watch::Receiver<Arc<ProviderUsageList>>,
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
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
            RuntimeMethod::IntegrationsRotateKey => {
                rotate_integration_key(state, composed, id, params).await
            }
            RuntimeMethod::ProvidersList => providers_list(state, composed, providers, id, params),
            RuntimeMethod::ProvidersUsage => providers_usage(state, composed, usage, id, params),
            RuntimeMethod::ProvidersWatch => {
                providers_watch(state, composed, provider_updates, usage_updates, id, params)
            }
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
            RuntimeMethod::SessionsWatchIndex => {
                sessions_watch_index(state, composed, sessions, id, params)
            }
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
            | RuntimeMethod::SessionsSubmitBlocks
            | RuntimeMethod::SessionsSetModel
            | RuntimeMethod::SessionsSetMode
            | RuntimeMethod::SessionsWatchEvents
            | RuntimeMethod::SessionsInterrupt
            | RuntimeMethod::SessionsCool
            | RuntimeMethod::ApprovalsListPending
            | RuntimeMethod::ApprovalsRespond => {
                session_operation(
                    state, composed, sessions, asking, returning, method, id, params,
                )
                .await
            }
            RuntimeMethod::SessionsForget => {
                forget_session(state, composed, sessions, asking, returning, id, params).await
            }
            RuntimeMethod::SessionsDeleteNative | RuntimeMethod::SessionsArchiveNative => {
                mutate_native_session(state, composed, discovering, sessions, method, id, params)
                    .await
            }
            RuntimeMethod::TerminalsList
            | RuntimeMethod::TerminalsWatchIndex
            | RuntimeMethod::TerminalsOpen
            | RuntimeMethod::TerminalsAttach
            | RuntimeMethod::TerminalsAcquireControl
            | RuntimeMethod::TerminalsRenewControl
            | RuntimeMethod::TerminalsReleaseControl
            | RuntimeMethod::TerminalsWrite
            | RuntimeMethod::TerminalsResize
            | RuntimeMethod::TerminalsDetach
            | RuntimeMethod::TerminalsStop => Answer::plain(
                id,
                RuntimeErrorKind::CapabilityUnavailable,
                "the public terminal surface is not available in this Runtime generation",
            ),
            RuntimeMethod::Initialized
            | RuntimeMethod::Challenge
            | RuntimeMethod::ProvidersChanged
            | RuntimeMethod::ProvidersWatchEnded
            | RuntimeMethod::ProvidersUsageChanged
            | RuntimeMethod::SessionsEvent
            | RuntimeMethod::SessionsLagged
            | RuntimeMethod::SessionsIndexChanged
            | RuntimeMethod::SessionsIndexEnded
            | RuntimeMethod::TerminalsIndexChanged
            | RuntimeMethod::TerminalsIndexEnded
            | RuntimeMethod::TerminalsOutput
            | RuntimeMethod::TerminalsLagged
            | RuntimeMethod::TerminalsExited => Answer::plain(
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
        RuntimeMethod::ProvidersList
        | RuntimeMethod::ProvidersWatch
        | RuntimeMethod::ProvidersUsage
        | RuntimeMethod::ProvidersGetCapabilities => Some(AppScope::ProviderRead),
        RuntimeMethod::ProvidersListModels => Some(AppScope::ModelRead),
        RuntimeMethod::ProvidersListNativeSessions => Some(AppScope::SessionNativeDiscover),
        RuntimeMethod::SessionsList
        | RuntimeMethod::SessionsWatchIndex
        | RuntimeMethod::SessionsGet
        | RuntimeMethod::TerminalsList
        | RuntimeMethod::TerminalsWatchIndex => Some(AppScope::SessionList),
        RuntimeMethod::SessionsStart => Some(AppScope::SessionStart),
        RuntimeMethod::SessionsAdoptNative | RuntimeMethod::SessionsResume => {
            Some(AppScope::SessionResume)
        }
        RuntimeMethod::SessionsAcquireControl
        | RuntimeMethod::SessionsSubmitInput
        | RuntimeMethod::SessionsSubmitBlocks
        | RuntimeMethod::SessionsSetModel
        | RuntimeMethod::SessionsSetMode
        | RuntimeMethod::TerminalsAcquireControl
        | RuntimeMethod::TerminalsRenewControl
        | RuntimeMethod::TerminalsReleaseControl
        | RuntimeMethod::TerminalsWrite
        | RuntimeMethod::TerminalsResize
        | RuntimeMethod::TerminalsStop => Some(AppScope::SessionInputWrite),
        RuntimeMethod::SessionsWatchEvents
        | RuntimeMethod::ApprovalsListPending
        | RuntimeMethod::TerminalsOpen
        | RuntimeMethod::TerminalsAttach
        | RuntimeMethod::TerminalsDetach => Some(AppScope::SessionOutputRead),
        RuntimeMethod::SessionsInterrupt | RuntimeMethod::SessionsCool => {
            Some(AppScope::SessionStop)
        }
        RuntimeMethod::SessionsForget
        | RuntimeMethod::SessionsDeleteNative
        | RuntimeMethod::SessionsArchiveNative => Some(AppScope::SessionDelete),
        RuntimeMethod::ApprovalsRespond
        | RuntimeMethod::SessionsRenewControl
        | RuntimeMethod::SessionsReleaseControl
        | RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::IntegrationsRotateKey
        | RuntimeMethod::ProvidersChanged
        | RuntimeMethod::ProvidersWatchEnded
        | RuntimeMethod::ProvidersUsageChanged
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::SessionsIndexChanged
        | RuntimeMethod::SessionsIndexEnded
        | RuntimeMethod::TerminalsIndexChanged
        | RuntimeMethod::TerminalsIndexEnded
        | RuntimeMethod::TerminalsOutput
        | RuntimeMethod::TerminalsLagged
        | RuntimeMethod::TerminalsExited
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
            build_digest: crate::build_identity::build_digest().map(str::to_owned),
        },
        server_capabilities: RuntimeCapabilities {
            integration_enrollment: true,
            provider_inventory: true,
            managed_session_list: true,
            model_discovery: true,
            native_session_catalogue: true,
            session_control: true,
            session_events: true,
            terminal_surface: false,
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

async fn rotate_integration_key(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<RotateIntegrationKeyParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration key rotation parameters are invalid",
        );
    };
    let authority = match authorized(state, &composed.store, None) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let row = match composed.store.get_integration(authority.key) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Unauthenticated,
                "the integration grant no longer exists",
            );
        }
        Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not read integration authority",
            );
        }
    };
    let new_public_key = match crate::runtime_auth::verify_key_rotation(&authority, &row, &params) {
        Ok(key) => key,
        Err(failure) => return Answer::failure(id, failure),
    };
    if row.key_generation == params.expected_key_generation + 1 && row.public_key == new_public_key
    {
        return match crate::runtime_auth::grant(authority.grant.integration_id, &row) {
            Ok(grant) => Answer::success(id, &grant),
            Err(failure) => Answer::failure(id, failure),
        };
    }
    let confirmation = composed
        .integration_admin
        .request_key_rotation_confirmation(
            authority.key,
            &params.request_id,
            params.expected_key_generation,
            new_public_key,
        )
        .await;
    match confirmation {
        Ok(crate::integration_admin::Confirmation::Awaiting { confirmation_id }) => {
            return Answer::operator_action(
                id,
                RuntimeErrorKind::PresenceRequired,
                "approve the exact integration key replacement in Runtrol Studio, then retry this request",
                "reviewRuntimeRequestsInRuntrolStudio",
                confirmation_id.into(),
            );
        }
        Ok(crate::integration_admin::Confirmation::Confirmed) => {}
        Err(failure) => return confirmation_failure(id, failure, "key rotation"),
    }
    key_rotation_answer(
        id,
        authority.grant.integration_id,
        composed.store.rotate_integration_key(
            authority.key,
            params.expected_key_generation,
            new_public_key,
        ),
    )
}

fn key_rotation_answer(
    id: JsonRpcId,
    integration_id: runtrol_runtime_protocol::IntegrationId,
    outcome: Result<IntegrationKeyRotation, runtrol_store::StoreError>,
) -> Answer {
    match outcome {
        Ok(IntegrationKeyRotation::Rotated(row)) => {
            match crate::runtime_auth::grant(integration_id, &row) {
                Ok(grant) => Answer::success_and_close(id, &grant),
                Err(failure) => Answer::failure(id, failure),
            }
        }
        Ok(IntegrationKeyRotation::Replayed(row)) => {
            match crate::runtime_auth::grant(integration_id, &row) {
                Ok(grant) => Answer::success(id, &grant),
                Err(failure) => Answer::failure(id, failure),
            }
        }
        Ok(IntegrationKeyRotation::Conflict) => Answer::plain(
            id,
            RuntimeErrorKind::IdempotencyConflict,
            "the integration key generation changed before this rotation committed",
        ),
        Ok(IntegrationKeyRotation::Missing) => Answer::plain(
            id,
            RuntimeErrorKind::Unauthenticated,
            "the integration grant no longer exists",
        ),
        Ok(IntegrationKeyRotation::Revoked) => Answer::failure(
            id,
            AuthorizationFailure {
                kind: RuntimeErrorKind::IntegrationRevoked,
                message: "the integration grant was revoked",
            },
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not rotate the integration key",
        ),
    }
}

fn providers_list(
    state: &mut PublicState,
    composed: &Composed,
    providers: &ProviderList,
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
        Ok(_) => Answer::success(id, providers),
        Err(failure) => Answer::failure(id, failure),
    }
}

/// Where each account stands against its limits, from the supervisor's latest snapshot.
///
/// Answered from a snapshot the serve task publishes when a report passes, so this read costs no lock on the
/// session owner and no provider process. An empty list means nothing has reported since the Runtime started,
/// which a surface says as "no report yet" rather than as a green light.
fn providers_usage(
    state: &mut PublicState,
    composed: &Composed,
    usage: &ProviderUsageList,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider usage parameters are invalid",
        );
    }
    match authorized(state, &composed.store, Some(AppScope::ProviderRead)) {
        Ok(_) => Answer::success(id, usage),
        Err(failure) => Answer::failure(id, failure),
    }
}

fn providers_watch(
    state: &mut PublicState,
    composed: &Composed,
    updates: &watch::Sender<Arc<ProviderList>>,
    usage: &watch::Receiver<Arc<ProviderUsageList>>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<WatchProvidersParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "provider watch parameters are invalid",
        );
    }
    let authority = match authorized(state, &composed.store, Some(AppScope::ProviderRead)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let subscription_id = match random_subscription_id() {
        Ok(subscription_id) if !subscription_id.is_empty() => subscription_id,
        Ok(_) | Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not allocate a provider subscription identity",
            );
        }
    };
    let provider_updates = updates.subscribe();
    let snapshot = provider_updates.borrow().as_ref().clone();
    Answer::watching_providers(
        id,
        &WatchProvidersResult {
            subscription_id,
            snapshot,
        },
        provider_updates,
        usage.clone(),
        authority,
    )
}

async fn get_provider_capabilities(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
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
            let _lane = discovering.lane(provider_id).await.lock_owned().await;
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
        set_model: Some(provider_capability(capabilities.set_model)),
        set_reasoning_effort: Some(provider_capability(capabilities.set_reasoning_effort)),
        native_session_delete: Some(provider_capability(capabilities.native_session_delete)),
        native_session_archive: Some(provider_capability(capabilities.native_session_archive)),
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
    discovering: &crate::serve::DiscoveryGates,
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
            let _lane = discovering.lane(provider_id).await.lock_owned().await;
            let prepared = crate::provider_prepare::prepared_driver(composed, provider_id)
                .await
                .map_err(|_| ())?;
            // Memoized against the exact binary for a bounded moment, so a picker opening twice
            // costs one provider spawn, not two.
            crate::provider_prepare::cached_models(composed, provider_id, &prepared)
                .await
                .map_err(|_| ())
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
    discovering: &crate::serve::DiscoveryGates,
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
    // A named folder still has to be one this integration holds. No folder means the machine, and
    // there is nothing to authorize against: the Runtime endpoint is owner-only local, the phone
    // speaks a different wire that has no native-discovery request at all, and the managed session
    // index already made exactly this move for exactly this reason (`runtime_inventory::authorized`,
    // folderless rule in `docs/runtimeProtocol.md`). What remains bounded is what the caller is
    // shown: every returned row is re-checked below before it reaches anyone.
    let selected_root = match params.root.as_deref() {
        Some(requested) => match crate::runtime_inventory::authorized_root(&authority, requested) {
            Ok(root) => Some(root),
            Err(failure) => return inventory_failure(id, failure),
        },
        None => None,
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
            // Held only through preparation, which is the region the probe-cache atomicity argument covers.
            // The listing itself spawns a whole CLI and can take seconds; holding this mutex across it queued
            // every provider's catalogue behind whichever CLI was slowest (measured 2026-08-19: a just-opened
            // folder's conversations arrived after 13~17 s serialized, ~1.2 s not). Concurrency of the listing
            // children is bounded by the semaphore below instead.
            let prepared = {
                let _lane = discovering.lane(provider).await.lock_owned().await;
                crate::provider_prepare::prepared_driver(composed, provider)
                    .await
                    .map_err(|_| NativeDiscoveryFailure::Provider)?
            };
            let _listing = discovering
                .listing
                .acquire()
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            // A driver that cannot enumerate the machine is never handed a folderless query. It
            // would have to either invent a folder or answer one folder's worth as if it were all,
            // and the second is how a partial list comes to read as complete.
            if selected_root.is_none() && !prepared.driver.enumerates_machine() {
                return Err(NativeDiscoveryFailure::RootRequired);
            }
            let opened = params
                .cursor
                .as_deref()
                .map(|cursor| {
                    native_cursors.open(
                        &authority,
                        provider,
                        selected_root.as_ref(),
                        prepared.binary_identity,
                        cursor,
                    )
                })
                .transpose()
                .map_err(NativeDiscoveryFailure::Cursor)?;
            let catalogue = prepared
                .driver
                .native_sessions(runtrol_provider::NativeSessionQuery {
                    root: selected_root.as_ref().map(|root| root.path.clone()),
                    cursor: opened.as_ref().map(|cursor| cursor.provider_cursor.clone()),
                    limit: MAX_PAGE_ITEMS,
                })
                .await
                .map_err(|_| NativeDiscoveryFailure::Provider)?;
            let next = catalogue.next_cursor.clone();
            let mut public = crate::runtime_native_sessions::authorize_catalogue(
                native_cursors,
                &authority,
                selected_root.as_ref(),
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
                            selected_root.as_ref(),
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
        // Named as its own failure so a caller can act on it: ask this provider per folder
        // instead. A generic refusal would leave it guessing which of the two it was.
        Ok(Err(NativeDiscoveryFailure::RootRequired)) => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "this provider lists conversations one workspace root at a time, so a root is required",
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
    /// The caller asked about the machine and this provider only answers about a folder.
    RootRequired,
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
        runtrol_provider::ModelCatalog::Aliases {
            aliases,
            reasoning_efforts,
            why,
        } => RuntimeModelCatalog::Aliases {
            aliases: aliases.into_iter().map(String::from).collect(),
            reasoning_efforts: reasoning_efforts
                .into_iter()
                .map(reasoning_choice)
                .collect(),
            why: String::from(why),
        },
        runtrol_provider::ModelCatalog::Partial {
            aliases,
            models,
            reasoning_efforts,
            why,
        } => RuntimeModelCatalog::Partial {
            aliases: aliases.into_iter().map(String::from).collect(),
            models: models.into_iter().map(model_choice).collect(),
            reasoning_efforts: reasoning_efforts
                .into_iter()
                .map(reasoning_choice)
                .collect(),
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

fn reasoning_choice(choice: runtrol_provider::ReasoningChoice) -> RuntimeReasoningChoice {
    RuntimeReasoningChoice {
        id: String::from(choice.id),
        description: String::from(choice.description),
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

fn sessions_watch_index(
    state: &mut PublicState,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<WatchSessionIndexParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "session index watch parameters are invalid",
        );
    }
    let authority = match authorized(state, &composed.store, Some(AppScope::SessionList)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let snapshot = match sessions.authorized(&authority) {
        Ok(snapshot) => snapshot,
        Err(failure) => return inventory_failure(id, failure),
    };
    let subscription_id = match random_subscription_id() {
        Ok(subscription_id) if !subscription_id.is_empty() => subscription_id,
        Ok(_) | Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not allocate a session index subscription identity",
            );
        }
    };
    Answer::watching_index(
        id,
        &WatchSessionIndexResult {
            subscription_id,
            snapshot,
        },
        authority,
    )
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
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
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
    let approval_scopes = ApprovalScopes {
        low: authority
            .grant
            .scopes
            .contains(&AppScope::ApprovalRespondLow),
        high: authority
            .grant
            .scopes
            .contains(&AppScope::ApprovalRespondHigh),
    };
    if method == RuntimeMethod::ApprovalsRespond && !approval_scopes.low && !approval_scopes.high {
        return Answer::plain(
            id,
            RuntimeErrorKind::ScopeDenied,
            "the integration lacks an approval response scope",
        );
    }
    let session = match sessions.authorized_session(&authority, parsed.session_id()) {
        Ok(session) => session,
        Err(failure) => return inventory_failure(id, failure),
    };
    if let ParsedSessionOperation::SetMode(mode_params) = &parsed
        && let Err(message) =
            mode_within_provider_vocabulary(composed, sessions, session, &mode_params.mode)
    {
        return Answer::plain(id, RuntimeErrorKind::InvalidRequest, message);
    }
    let request = parsed.into_owner_request(session, approval_scopes);
    let (answered, hearing) = oneshot::channel();
    if asking
        .send(Box::new(RuntimeAsked {
            integration: authority.key,
            request,
            answered,
        }))
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

/// Whether this provider accepts a runtrol switch to the named mode.
///
/// The manifest's `switchable` list is the boundary for a CLI whose vocabulary cannot be discovered, and it
/// deliberately omits the modes that remove safety prompts, so those are unreachable through this method for
/// every caller. An empty list means the protocol announces modes per session, and the driver itself gates on
/// that announcement (measured: one agent confirms unannounced switches with an empty success, which is why
/// somebody must gate). A session whose provider cannot be identified is refused rather than relayed.
fn mode_within_provider_vocabulary(
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    session: runtrol_provider::SessionId,
    mode: &str,
) -> Result<(), &'static str> {
    let Some(provider) = sessions.provider_of(session) else {
        return Err("the session's provider cannot be identified for a mode switch");
    };
    mode_within_manifest_vocabulary(composed, provider, mode)
}

/// The manifest half of the mode boundary, shared by mid-session switching and session start.
///
/// One definition on purpose: starting a session at a mode must never reach anything that switching
/// a session to it could not, or the start path becomes the way around the safety boundary.
fn mode_within_manifest_vocabulary(
    composed: &Composed,
    provider: runtrol_provider::ProviderId,
    mode: &str,
) -> Result<(), &'static str> {
    let Some(entry) = composed.registry.get(provider) else {
        return Err("the session's provider is not registered in this build");
    };
    let switchable = &entry.manifest.modes.switchable;
    if switchable.is_empty() || switchable.iter().any(|token| **token == *mode) {
        return Ok(());
    }
    Err("this provider does not accept a runtrol switch to that mode")
}

async fn forget_session(
    state: &mut PublicState,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<ForgetSessionParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "session forget parameters are invalid",
        );
    };
    let authority = match authorized(state, &composed.store, Some(AppScope::SessionDelete)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let Ok(session) = params
        .session_id
        .as_str()
        .parse::<runtrol_provider::SessionId>()
    else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the Runtime session identity is invalid",
        );
    };
    let confirmation = match sessions.authorized_descriptor(&authority, &params.session_id) {
        Ok(descriptor) => {
            if descriptor.lifecycle != runtrol_runtime_protocol::LifecycleState::Cold
                || descriptor.session_generation != params.expected_session_generation
            {
                return Answer::plain(
                    id,
                    RuntimeErrorKind::SessionConflict,
                    "only the exact observed cold session pointer can be forgotten",
                );
            }
            composed
                .integration_admin
                .request_forget_confirmation(
                    authority.key,
                    &params.request_id,
                    session,
                    params.expected_session_generation,
                )
                .await
        }
        Err(RuntimeInventoryFailure::SessionNotFound) => {
            match composed
                .integration_admin
                .existing_forget_confirmation(
                    authority.key,
                    &params.request_id,
                    session,
                    params.expected_session_generation,
                )
                .await
            {
                Ok(Some(crate::integration_admin::Confirmation::Confirmed)) => {
                    Ok(crate::integration_admin::Confirmation::Confirmed)
                }
                Ok(Some(crate::integration_admin::Confirmation::Awaiting { .. }) | None) => {
                    return inventory_failure(id, RuntimeInventoryFailure::SessionNotFound);
                }
                Err(failure) => return confirmation_failure(id, failure, "session forget"),
            }
        }
        Err(failure) => return inventory_failure(id, failure),
    };
    match confirmation {
        Ok(crate::integration_admin::Confirmation::Awaiting { confirmation_id }) => {
            return Answer::operator_action(
                id,
                RuntimeErrorKind::PresenceRequired,
                "approve the exact metadata removal in Runtrol Studio, then retry this request",
                "reviewRuntimeRequestsInRuntrolStudio",
                confirmation_id.into(),
            );
        }
        Ok(crate::integration_admin::Confirmation::Confirmed) => {}
        Err(failure) => return confirmation_failure(id, failure, "session forget"),
    }
    let (answered, hearing) = oneshot::channel();
    if asking
        .send(Box::new(RuntimeAsked {
            integration: authority.key,
            request: RuntimeControlRequest::Forget { session, params },
            answered,
        }))
        .await
        .is_err()
    {
        return runtime_owner_stopped(id);
    }
    match hearing.await {
        Ok(reply) => runtime_control_answer(id, reply, returning).await,
        Err(_) => runtime_owner_stopped(id),
    }
}

#[derive(Clone, Copy)]
enum NativeSessionMutation {
    Delete,
    Archive,
}

impl NativeSessionMutation {
    fn from_method(method: RuntimeMethod) -> Option<Self> {
        match method {
            RuntimeMethod::SessionsDeleteNative => Some(Self::Delete),
            RuntimeMethod::SessionsArchiveNative => Some(Self::Archive),
            _ => None,
        }
    }

    fn timeout_message(self) -> &'static str {
        match self {
            Self::Delete => "the provider did not answer the deletion within its bounded deadline",
            Self::Archive => "the provider did not answer the archival within its bounded deadline",
        }
    }
}

struct NativeSessionMutationParams {
    provider_id: runtrol_runtime_protocol::ProviderId,
    native_session_id: String,
    workspace: String,
}

fn parse_native_session_mutation(
    method: RuntimeMethod,
    params: serde_json::Value,
) -> Result<(NativeSessionMutation, NativeSessionMutationParams), &'static str> {
    let Some(mutation) = NativeSessionMutation::from_method(method) else {
        return Err("native session mutation method is invalid");
    };
    let parsed = match mutation {
        NativeSessionMutation::Delete => {
            let value = serde_json::from_value::<DeleteNativeSessionParams>(params)
                .map_err(|_| "native session deletion parameters are invalid")?;
            NativeSessionMutationParams {
                provider_id: value.provider_id,
                native_session_id: value.native_session_id,
                workspace: value.workspace,
            }
        }
        NativeSessionMutation::Archive => {
            let value = serde_json::from_value::<ArchiveNativeSessionParams>(params)
                .map_err(|_| "native session archival parameters are invalid")?;
            NativeSessionMutationParams {
                provider_id: value.provider_id,
                native_session_id: value.native_session_id,
                workspace: value.workspace,
            }
        }
    };
    Ok((mutation, parsed))
}

/// Mutate one provider-native conversation through the provider's own surface.
///
/// Runtime verifies the folder and refuses while it supervises the conversation, then asks the CLI
/// that owns it. Runtime changes no provider store itself and retains no transcript copy.
async fn mutate_native_session(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    sessions: &RuntimeSessionCatalogue,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let (mutation, params) = match parse_native_session_mutation(method, params) {
        Ok(parsed) => parsed,
        Err(message) => return Answer::plain(id, RuntimeErrorKind::InvalidRequest, message),
    };
    let authority = match authorized(state, &composed.store, Some(AppScope::SessionDelete)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let Ok(provider) = runtrol_provider::ProviderId::parse(params.provider_id.as_str()) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the selected provider identity is invalid",
        );
    };
    let Ok(native) = runtrol_provider::NativeSessionId::new(&params.native_session_id) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "native session identity is invalid",
        );
    };
    let workspace = match authorized_workspace(&authority, &params.workspace) {
        Ok(workspace) => workspace.path,
        Err(failure) => return inventory_failure(id, failure),
    };
    match sessions.managed_as(&authority, provider, &native) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::SessionConflict,
                "Runtime supervises this conversation; forget the supervised session first",
            );
        }
        Err(failure) => return inventory_failure(id, failure),
    }
    let prepared = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            let _lane = discovering.lane(provider).await.lock_owned().await;
            crate::provider_prepare::driver(composed, provider).await
        },
    )
    .await;
    let driver = match prepared {
        Ok(Ok(driver)) => driver,
        Ok(Err(_)) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider could not be prepared",
            );
        }
        Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::RuntimeUnavailable,
                "provider preparation exceeded its bounded deadline",
            );
        }
    };
    // The asked identity survives the provider's answer: a deletion then forgets Runtrol's pointers to it.
    let asked = native.clone();
    let mutated = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        async {
            match mutation {
                NativeSessionMutation::Delete => {
                    driver
                        .delete_native_session(runtrol_provider::NativeSessionDeletion {
                            native: asked,
                            cwd: workspace,
                        })
                        .await
                }
                NativeSessionMutation::Archive => {
                    driver
                        .archive_native_session(runtrol_provider::NativeSessionArchival {
                            native: asked,
                            cwd: workspace,
                        })
                        .await
                }
            }
        },
    )
    .await;
    match mutated {
        Ok(Ok(())) => mutated_answer(id, mutation, composed, provider, &native),
        // The provider's own answer, by kind: unsupported stays unsupported, a refusal stays a refusal.
        Ok(Err(error)) => control_failure(id, crate::runtime_control::provider_failure(&error)),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            mutation.timeout_message(),
        ),
    }
}

/// The answer after the provider agreed, with Runtrol's own bookkeeping done.
///
/// A deleted conversation must not linger as a nameless Runtrol pointer: the pointer names nothing and
/// the operator can neither open it nor delete it again. Measured 2026-08-25: two such rows sat in the
/// sidebar after two deletions. An archive keeps its pointers; the conversation still exists.
fn mutated_answer(
    id: JsonRpcId,
    mutation: NativeSessionMutation,
    composed: &Composed,
    provider: runtrol_provider::ProviderId,
    native: &runtrol_provider::NativeSessionId,
) -> Answer {
    if !matches!(mutation, NativeSessionMutation::Delete) {
        return Answer::success(id, &serde_json::json!({}));
    }
    if let Err(error) = forget_pointers_of(&composed.store, provider, native) {
        return Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            &format!(
                "the coding service deleted the conversation, but Runtrol could not forget its own pointer to it: {error}"
            ),
        );
    }
    Answer::success(id, &serde_json::json!({}))
}

/// Drop every stored session pointer that named this provider conversation. Only pointers without a
/// live process can be here: a supervised one was refused above before the provider was asked.
fn forget_pointers_of(
    store: &runtrol_store::Store,
    provider: runtrol_provider::ProviderId,
    native: &runtrol_provider::NativeSessionId,
) -> Result<usize, runtrol_store::StoreError> {
    let listed = store.list_sessions()?;
    let mut forgotten = 0;
    for (session, row) in listed.sessions {
        if row.provider == provider && &row.native == native && store.remove_session(session)? {
            forgotten += 1;
        }
    }
    Ok(forgotten)
}

/// A presence-confirmation table refusing a request, said in the caller's own terms (`what` is the
/// operation: "session forget", "key rotation", "shared session open").
fn confirmation_failure(
    id: JsonRpcId,
    failure: crate::integration_admin::ConfirmationError,
    what: &str,
) -> Answer {
    use crate::integration_admin::ConfirmationError;
    match failure {
        ConfirmationError::InvalidRequestId => Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            &format!("the {what} request identity is invalid or outside its bounded lifetime"),
        ),
        ConfirmationError::IdempotencyConflict => Answer::plain(
            id,
            RuntimeErrorKind::IdempotencyConflict,
            &format!("the {what} request identity was already bound to different parameters"),
        ),
        ConfirmationError::ResourceExhausted => Answer::plain(
            id,
            RuntimeErrorKind::ResourceExhausted,
            &format!("too many {what} requests are awaiting local confirmation"),
        ),
        ConfirmationError::StateUnavailable => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            &format!("Runtime could not verify local {what} confirmation state"),
        ),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the public open boundary keeps closed parsing, live authority, workspace identity, and owner reservation ordering together"
)]
async fn open_session(
    state: &mut PublicState,
    composed: &Composed,
    discovering: &crate::serve::DiscoveryGates,
    native_cursors: &NativeCursorCodec,
    sessions: &RuntimeSessionCatalogue,
    asking: &mpsc::Sender<Box<RuntimeAsked>>,
    returning: &mpsc::UnboundedSender<RuntimeReturned>,
    method: RuntimeMethod,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let authority = match authorized(state, &composed.store, required_scope(method)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let request = match build_open_request(method, params, &authority, sessions, composed).await {
        Ok(request) => request,
        Err(OpenAdmissionFailure::Control(failure)) => return control_failure(id, failure),
        Err(OpenAdmissionFailure::Inventory(failure)) => return inventory_failure(id, failure),
        Err(OpenAdmissionFailure::Presence { confirmation_id }) => {
            return Answer::operator_action(
                id,
                RuntimeErrorKind::PresenceRequired,
                "approve the exact shared-writer session open in Runtrol Studio, then retry this request",
                "reviewRuntimeRequestsInRuntrolStudio",
                confirmation_id.into(),
            );
        }
        Err(OpenAdmissionFailure::Confirmation(failure)) => {
            return confirmation_failure(id, failure, "shared session open");
        }
    };
    let (answered, hearing) = oneshot::channel();
    if asking
        .send(Box::new(RuntimeAsked {
            integration: authority.key,
            request: RuntimeControlRequest::PrepareOpen(request),
            answered,
        }))
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
        | RuntimeControlReply::Approvals(_)
        | RuntimeControlReply::Watching { .. }
        | RuntimeControlReply::Sending { .. }
        | RuntimeControlReply::Cooling(_) => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "the session owner returned a mismatched open response",
        ),
    }
}

enum OpenAdmissionFailure {
    Control(RuntimeControlFailure),
    Inventory(RuntimeInventoryFailure),
    /// A shared-writer open waiting for the person at the machine; the id names the exact request.
    Presence {
        confirmation_id: Box<str>,
    },
    /// The presence table refused to remember the request.
    Confirmation(crate::integration_admin::ConfirmationError),
}

#[expect(
    clippy::too_many_lines,
    reason = "closed start, adoption, and resume shapes share one explicit authority-to-owner admission boundary"
)]
async fn build_open_request(
    method: RuntimeMethod,
    params: serde_json::Value,
    authority: &AuthorizedIntegration,
    sessions: &RuntimeSessionCatalogue,
    composed: &Composed,
) -> Result<RuntimeOpenRequest, OpenAdmissionFailure> {
    match method {
        RuntimeMethod::SessionsStart => {
            let params: StartSessionParams = serde_json::from_value(params).map_err(|_| {
                OpenAdmissionFailure::Control(invalid_open("session start parameters are invalid"))
            })?;
            let provider = parse_open_provider(&params.provider_id)?;
            let workspace = authorized_workspace(authority, &params.workspace)
                .map_err(OpenAdmissionFailure::Inventory)?;
            let access = open_access(
                composed,
                authority,
                &params.request_id,
                method,
                provider,
                &workspace.path,
                params.access,
            )
            .await?;
            let model = params.model.map(validate_model_selection).transpose()?;
            let reasoning_effort = params
                .reasoning_effort
                .map(validate_reasoning_selection)
                .transpose()?;
            let permission = params
                .permission
                .map(validate_permission_selection)
                .transpose()?;
            // The same boundary the mid-session switch enforces: an empty vocabulary means the
            // protocol announces modes per session and the driver gates, a non-empty one is the
            // measured switchable set with the safety-removing modes deliberately absent.
            if let Some(mode) = permission.as_deref() {
                mode_within_manifest_vocabulary(composed, provider, mode)
                    .map_err(|why| OpenAdmissionFailure::Control(invalid_open(why)))?;
            }
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
                reasoning_effort,
                permission,
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
            let access = open_access(
                composed,
                authority,
                &params.request_id,
                method,
                provider,
                &workspace.path,
                params.access,
            )
            .await?;
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
                reasoning_effort: None,
                permission: None,
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
            let access = open_access(
                composed,
                authority,
                &params.request_id,
                method,
                managed.provider,
                &workspace.path,
                params.access,
            )
            .await?;
            // Resume takes the same two optional selections as start: the codex driver already rides
            // them on thread/resume and the claude driver attaches its flags regardless of
            // disposition, so only this admission path was withholding them.
            let model = params.model.map(validate_model_selection).transpose()?;
            let reasoning_effort = params
                .reasoning_effort
                .map(validate_reasoning_selection)
                .transpose()?;
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
                model,
                reasoning_effort,
                permission: None,
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

/// Writer collision posture for one public open. Exclusive is every caller's right. Shared is a decision
/// for the person at the machine, so it waits in the presence table until they confirm this exact
/// request (Runtrol Studio confirms its own click; anyone else's request is shown to the operator), and
/// the unchanged retry then passes.
async fn open_access(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    request_id: &runtrol_runtime_protocol::MutationRequestId,
    method: RuntimeMethod,
    provider: runtrol_provider::ProviderId,
    workspace: &runtrol_provider::AbsPath,
    access: SessionWorkspaceAccess,
) -> Result<WorkspaceAccess, OpenAdmissionFailure> {
    match access {
        SessionWorkspaceAccess::Exclusive => Ok(WorkspaceAccess::Exclusive),
        SessionWorkspaceAccess::Shared => {
            let subject = crate::integration_admin::SharedOpenSubject {
                method,
                provider,
                workspace: workspace.clone(),
            };
            match composed
                .integration_admin
                .request_shared_open_confirmation(authority.key, request_id, subject)
                .await
            {
                Ok(crate::integration_admin::Confirmation::Confirmed) => {
                    Ok(WorkspaceAccess::Shared)
                }
                Ok(crate::integration_admin::Confirmation::Awaiting { confirmation_id }) => {
                    Err(OpenAdmissionFailure::Presence { confirmation_id })
                }
                Err(failure) => Err(OpenAdmissionFailure::Confirmation(failure)),
            }
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

fn validate_reasoning_selection(value: String) -> Result<Box<str>, OpenAdmissionFailure> {
    if value.is_empty()
        || value.len() > runtrol_runtime_protocol::MAX_REASONING_SELECTION_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(OpenAdmissionFailure::Control(invalid_open(
            "the reasoning selection is empty, oversized, or invalid",
        )));
    }
    Ok(value.into())
}

fn validate_permission_selection(value: String) -> Result<Box<str>, OpenAdmissionFailure> {
    if value.is_empty()
        || value.len() > runtrol_runtime_protocol::MAX_PERMISSION_SELECTION_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(OpenAdmissionFailure::Control(invalid_open(
            "the permission selection is empty, oversized, or invalid",
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
    discovering: &crate::serve::DiscoveryGates,
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
        let _lane = discovering.lane(provider).await.lock_owned().await;
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
    let selected_model = guard.opening().and_then(|opening| opening.model.as_deref());
    let selected_effort = guard
        .opening()
        .and_then(|opening| opening.reasoning_effort.as_deref());
    if selected_model.is_some() || selected_effort.is_some() {
        // The same bounded memoization the public listing uses, so validating a start no longer
        // re-spawns the provider the picker just asked.
        let discovered =
            crate::provider_prepare::cached_models(composed, provider, &prepared).await;
        let choices_are_current = discovered.is_ok_and(|catalogue| {
            selected_model.is_none_or(|model| model_is_current(&catalogue, model))
                && selected_effort.is_none_or(|effort| {
                    reasoning_effort_is_current(&catalogue, selected_model, effort)
                })
        });
        if !choices_are_current {
            return send_open_denied(
                id,
                guard,
                returning,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::ModelUnavailable,
                    "the selected model or reasoning effort is not present in the provider's current catalogue",
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
            reasoning_effort: opening.reasoning_effort.clone(),
            permission: opening.permission.clone(),
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
        // The provider said no, and said why. That is a denial with its own kind (not installed, not
        // signed in, over quota, unsupported, unreadable), not an unknown outcome: nothing may have
        // happened, and the caller's next move depends entirely on which it was. Measured 2026-08-21:
        // every adoption refusal reached the sidebar as "the mutation may have happened", and the real
        // reasons (two driver bounds) were only found by probing the drivers directly.
        Ok(Err(error)) => {
            send_open_denied(
                id,
                guard,
                returning,
                crate::runtime_control::provider_failure(&error),
            )
            .await
        }
        // Only the deadline passing is genuinely unknown: the provider may still be opening.
        Err(_) => send_open_unknown(id, guard, returning).await,
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

fn reasoning_effort_is_current(
    catalogue: &runtrol_provider::ModelCatalog,
    selected_model: Option<&str>,
    selected_effort: &str,
) -> bool {
    let model_efforts = match (catalogue, selected_model) {
        (
            runtrol_provider::ModelCatalog::Known { models }
            | runtrol_provider::ModelCatalog::Partial { models, .. },
            Some(selected),
        ) => models
            .iter()
            .find(|model| model.id.as_ref() == selected)
            .map(|model| model.reasoning_efforts.as_slice()),
        _ => None,
    };
    if model_efforts.is_some_and(|efforts| !efforts.is_empty()) {
        return model_efforts.is_some_and(|efforts| {
            efforts
                .iter()
                .any(|effort| effort.id.as_ref() == selected_effort)
        });
    }
    match catalogue {
        runtrol_provider::ModelCatalog::Aliases {
            reasoning_efforts, ..
        }
        | runtrol_provider::ModelCatalog::Partial {
            reasoning_efforts, ..
        } => reasoning_efforts
            .iter()
            .any(|effort| effort.id.as_ref() == selected_effort),
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
    SubmitBlocks(SubmitBlocksParams),
    SetModel(SetModelParams),
    SetMode(SetModeParams),
    Watch {
        params: WatchEventsParams,
        subscription_id: String,
    },
    Interrupt(ControlLeaseParams),
    Cool(CoolSessionParams),
    ListApprovals(ListPendingApprovalsParams),
    RespondApproval(RespondApprovalParams),
}

impl ParsedSessionOperation {
    fn session_id(&self) -> &RuntimeSessionId {
        match self {
            Self::Acquire(params) => &params.session_id,
            Self::Renew(params) | Self::Release(params) | Self::Interrupt(params) => {
                &params.session_id
            }
            Self::Submit(params) => &params.session_id,
            Self::SubmitBlocks(params) => &params.session_id,
            Self::SetModel(params) => &params.session_id,
            Self::SetMode(params) => &params.session_id,
            Self::Watch { params, .. } => &params.session_id,
            Self::Cool(params) => &params.session_id,
            Self::ListApprovals(params) => &params.session_id,
            Self::RespondApproval(params) => &params.session_id,
        }
    }

    fn into_owner_request(
        self,
        session: runtrol_provider::SessionId,
        scopes: ApprovalScopes,
    ) -> RuntimeControlRequest {
        match self {
            Self::Acquire(params) => RuntimeControlRequest::Acquire { session, params },
            Self::Renew(params) => RuntimeControlRequest::Renew { session, params },
            Self::Release(params) => RuntimeControlRequest::Release { session, params },
            Self::Submit(params) => RuntimeControlRequest::Submit { session, params },
            Self::SubmitBlocks(params) => RuntimeControlRequest::SubmitBlocks { session, params },
            Self::SetModel(params) => RuntimeControlRequest::SetModel { session, params },
            Self::SetMode(params) => RuntimeControlRequest::SetMode { session, params },
            Self::Watch {
                params,
                subscription_id,
            } => RuntimeControlRequest::Watch {
                session,
                params,
                subscription_id,
            },
            Self::Interrupt(params) => RuntimeControlRequest::Interrupt { session, params },
            Self::Cool(params) => RuntimeControlRequest::Cool { session, params },
            Self::ListApprovals(params) => RuntimeControlRequest::ListApprovals {
                session,
                params,
                scopes,
            },
            Self::RespondApproval(params) => RuntimeControlRequest::RespondApproval {
                session,
                params,
                authority: if scopes.high {
                    ApprovalAuthority::High
                } else {
                    ApprovalAuthority::Low
                },
            },
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
        RuntimeMethod::SessionsSubmitBlocks => serde_json::from_value(params)
            .map(ParsedSessionOperation::SubmitBlocks)
            .map_err(|_| "session block parameters are invalid"),
        RuntimeMethod::SessionsSetModel => serde_json::from_value(params)
            .map(ParsedSessionOperation::SetModel)
            .map_err(|_| "model switch parameters are invalid"),
        RuntimeMethod::SessionsSetMode => serde_json::from_value(params)
            .map(ParsedSessionOperation::SetMode)
            .map_err(|_| "mode switch parameters are invalid"),
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
        RuntimeMethod::SessionsCool => serde_json::from_value(params)
            .map(ParsedSessionOperation::Cool)
            .map_err(|_| "session cooling parameters are invalid"),
        RuntimeMethod::ApprovalsListPending => serde_json::from_value(params)
            .map(ParsedSessionOperation::ListApprovals)
            .map_err(|_| "pending approval list parameters are invalid"),
        RuntimeMethod::ApprovalsRespond => serde_json::from_value(params)
            .map(ParsedSessionOperation::RespondApproval)
            .map_err(|_| "approval response parameters are invalid"),
        RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::IntegrationsRotateKey
        | RuntimeMethod::ProvidersList
        | RuntimeMethod::ProvidersWatch
        | RuntimeMethod::ProvidersChanged
        | RuntimeMethod::ProvidersWatchEnded
        | RuntimeMethod::ProvidersUsageChanged
        | RuntimeMethod::ProvidersGetCapabilities
        | RuntimeMethod::ProvidersListModels
        | RuntimeMethod::ProvidersListNativeSessions
        | RuntimeMethod::SessionsList
        | RuntimeMethod::SessionsWatchIndex
        | RuntimeMethod::SessionsGet
        | RuntimeMethod::SessionsStart
        | RuntimeMethod::SessionsAdoptNative
        | RuntimeMethod::SessionsResume
        | RuntimeMethod::SessionsForget
        | RuntimeMethod::SessionsDeleteNative
        | RuntimeMethod::SessionsArchiveNative
        | RuntimeMethod::TerminalsList
        | RuntimeMethod::TerminalsWatchIndex
        | RuntimeMethod::TerminalsOpen
        | RuntimeMethod::TerminalsAttach
        | RuntimeMethod::TerminalsAcquireControl
        | RuntimeMethod::TerminalsRenewControl
        | RuntimeMethod::TerminalsReleaseControl
        | RuntimeMethod::TerminalsWrite
        | RuntimeMethod::TerminalsResize
        | RuntimeMethod::TerminalsDetach
        | RuntimeMethod::TerminalsStop
        | RuntimeMethod::TerminalsIndexChanged
        | RuntimeMethod::TerminalsIndexEnded
        | RuntimeMethod::TerminalsOutput
        | RuntimeMethod::TerminalsLagged
        | RuntimeMethod::TerminalsExited
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::SessionsIndexChanged
        | RuntimeMethod::SessionsIndexEnded
        | RuntimeMethod::ProvidersUsage
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
        RuntimeControlReply::Approvals(approvals) => Answer::success(id, &approvals),
        RuntimeControlReply::Watching { result, view } => {
            Answer::watching_events(id, &result, view)
        }
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
        RuntimeControlReply::Cooling(cooling) => {
            match perform_runtime_cool(cooling, returning.clone()).await {
                Some(Ok(())) => Answer::success(id, &EmptyResult {}),
                Some(Err(failure)) => control_failure(id, failure),
                None => Answer::plain(
                    id,
                    RuntimeErrorKind::RuntimeUnavailable,
                    "the Runtime session owner stopped",
                ),
            }
        }
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
async fn perform_runtime_cool(
    cooling: RuntimeCooling,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
) -> Option<Result<(), RuntimeControlFailure>> {
    let RuntimeCooling {
        mutation,
        agent: handed_agent,
        reservation,
    } = cooling;
    let guard = RuntimeCoolGuard::new(mutation, reservation, returning.clone());
    let agent = handed_agent;
    let outcome = agent.close(CloseMode::graceful()).await;
    let reservation = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(RuntimeReturned::Cooled {
            mutation,
            reservation,
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

async fn relay_watch(
    connection: &mut Connection,
    watching: Watching,
    sessions: &mut watch::Receiver<Arc<RuntimeSessionCatalogue>>,
    composed: &Composed,
) {
    match watching {
        Watching::Events {
            subscription_id,
            session_id,
            view,
        } => relay_events(connection, subscription_id, session_id, view).await,
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
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a detached inventory refresh has no waiting request to answer; stderr is the daemon's existing operational failure channel"
)]
fn schedule_provider_inventory_refresh(
    providers: watch::Sender<Arc<ProviderList>>,
    composed: Arc<Composed>,
) {
    drop(tokio::spawn(async move {
        match crate::runtime_inventory::providers_in_background(composed).await {
            Ok(Some(next)) => {
                let next = Arc::new(next);
                providers.send_if_modified(|current| {
                    if current.as_ref() == next.as_ref() {
                        return false;
                    }
                    *current = next;
                    true
                });
            }
            Ok(None) => {}
            Err(error) => eprintln!("{error}"),
        }
    }));
}

const fn method_needs_provider_refresh(method: RuntimeMethod) -> bool {
    matches!(
        method,
        RuntimeMethod::ProvidersList
            | RuntimeMethod::ProvidersWatch
            | RuntimeMethod::ProvidersGetCapabilities
            | RuntimeMethod::ProvidersListModels
            | RuntimeMethod::ProvidersListNativeSessions
            | RuntimeMethod::SessionsStart
            | RuntimeMethod::SessionsAdoptNative
            | RuntimeMethod::SessionsResume
    )
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
        match refresh(&composed.store, &authority) {
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
        let current_authority = match refresh(&composed.store, &authority) {
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

    #[test]
    fn native_archive_has_the_same_authority_and_exact_payload_boundary_as_delete() {
        let params = ArchiveNativeSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            provider_id: runtrol_runtime_protocol::ProviderId::new("codex"),
            native_session_id: "thread-1".to_owned(),
            workspace: "C:\\work\\alpha".to_owned(),
        };
        let (mutation, parsed) = parse_native_session_mutation(
            RuntimeMethod::SessionsArchiveNative,
            serde_json::to_value(params).expect("archive parameters serialize"),
        )
        .expect("archive parameters stay inside their public DTO");
        assert!(matches!(mutation, NativeSessionMutation::Archive));
        assert_eq!(parsed.native_session_id, "thread-1");
        assert_eq!(
            required_scope(RuntimeMethod::SessionsArchiveNative),
            Some(AppScope::SessionDelete),
        );
    }

    #[test]
    fn archive_capability_is_projected_without_guessing() {
        let provider_id = runtrol_runtime_protocol::ProviderId::new("provider-a");
        let projected = provider_capabilities(
            provider_id.clone(),
            runtrol_provider::ProviderCapabilities::unknown(),
        );
        assert_eq!(projected.provider_id, provider_id);
        assert!(matches!(
            projected.native_session_archive,
            Some(ProviderCapabilityObservation {
                availability: ProviderCapabilityAvailability::Unknown,
                ..
            })
        ));
    }

    /// After the provider deletes a conversation, every Runtrol pointer that named it goes too, and
    /// nothing else does. Measured 2026-08-25 before this: two deleted Claude conversations lingered
    /// as nameless rows that could be neither opened nor deleted again.
    #[test]
    fn deleting_a_native_conversation_forgets_only_its_own_pointers() {
        let scratch =
            std::env::temp_dir().join(format!("runtrol-forget-pointer-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let cwd =
            runtrol_provider::AbsPath::canonicalize(home).expect("the scratch home canonicalizes");
        let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
        let codex = runtrol_provider::ProviderId::parse("codex").expect("a builtin provider");
        let now = runtrol_provider::WallMs::now();
        let row = |provider, native: &str| runtrol_store::SessionRow {
            provider,
            native: runtrol_provider::NativeSessionId::new(native).expect("a valid native id"),
            cwd: cwd.clone(),
            label: None,
            created_at: now,
            last_seen_at: now,
            pinned: false,
            archived: false,
            forked_from: None,
            live: None,
        };
        let gone_a = runtrol_provider::SessionId::now();
        let gone_b = runtrol_provider::SessionId::now();
        let other_native = runtrol_provider::SessionId::now();
        let other_provider = runtrol_provider::SessionId::now();
        composed
            .store
            .put_session(gone_a, &row(claude, "deleted-one"))
            .expect("store");
        composed
            .store
            .put_session(gone_b, &row(claude, "deleted-one"))
            .expect("store");
        composed
            .store
            .put_session(other_native, &row(claude, "kept-one"))
            .expect("store");
        composed
            .store
            .put_session(other_provider, &row(codex, "deleted-one"))
            .expect("store");

        let deleted =
            runtrol_provider::NativeSessionId::new("deleted-one").expect("a valid native id");
        let forgotten =
            forget_pointers_of(&composed.store, claude, &deleted).expect("the store answers");
        assert_eq!(forgotten, 2, "both pointers to the deleted conversation go");
        let remaining: Vec<_> = composed
            .store
            .list_sessions()
            .expect("list")
            .sessions
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(remaining, vec![other_native, other_provider]);
        assert_eq!(
            forget_pointers_of(&composed.store, claude, &deleted).expect("the store answers"),
            0,
            "a second deletion finds nothing to forget"
        );
        std::fs::remove_dir_all(&scratch).expect("clean the scratch home");
    }

    #[test]
    fn a_mode_travels_only_within_the_provider_vocabulary() {
        // claude's manifest lists default, plan, and acceptEdits, and deliberately omits the modes that
        // remove safety prompts; grok's manifest has no mode list at all, which defers the gate to its
        // driver's session-announcement check. Both facts are what this test pins.
        let scratch =
            std::env::temp_dir().join(format!("runtrol-mode-gate-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let workspace =
            runtrol_provider::AbsPath::canonicalize(home).expect("the scratch home canonicalizes");

        let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
        let catalogue = crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
            claude,
            "native-mode-gate",
            &workspace,
        );
        let session = catalogue
            .first_session_id_for_tests()
            .expect("the fixture holds one session");
        assert!(
            mode_within_provider_vocabulary(&composed, &catalogue, session, "acceptEdits").is_ok(),
            "a manifest-listed mode travels"
        );
        assert!(
            mode_within_provider_vocabulary(&composed, &catalogue, session, "bypassPermissions")
                .is_err(),
            "the mode that removes every question must be unreachable through runtrol"
        );
        assert!(
            mode_within_provider_vocabulary(
                &composed,
                &catalogue,
                runtrol_provider::SessionId::now(),
                "default",
            )
            .is_err(),
            "an unidentifiable session fails closed"
        );

        let grok = runtrol_provider::ProviderId::parse("grok").expect("a builtin provider");
        let announced = crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
            grok,
            "native-mode-gate-acp",
            &workspace,
        );
        let acp_session = announced
            .first_session_id_for_tests()
            .expect("the fixture holds one session");
        assert!(
            mode_within_provider_vocabulary(&composed, &announced, acp_session, "anything").is_ok(),
            "an empty manifest list defers to the driver's session-announcement gate"
        );

        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }

    #[test]
    fn starting_at_a_permission_mode_passes_the_exact_switch_boundary() {
        // sessions/start validates its permission through the same manifest function the mid-session
        // switch uses, so starting a session can never reach a mode that switching one could not.
        // plan is in claude's switchable list; the modes that remove safety prompts are not.
        let scratch =
            std::env::temp_dir().join(format!("runtrol-start-mode-gate-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
        assert!(
            mode_within_manifest_vocabulary(&composed, claude, "plan").is_ok(),
            "a session can start in plan mode"
        );
        for dangerous in ["bypassPermissions", "dontAsk", "auto"] {
            assert!(
                mode_within_manifest_vocabulary(&composed, claude, dangerous).is_err(),
                "{dangerous} must be unreachable at start exactly as it is at switch"
            );
        }
        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }

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

        /// Stands in for the four measured providers that answer without a folder filter.
        fn enumerates_machine(&self) -> bool {
            true
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
                    // The fixture answers inside whatever folder it was asked about, and names
                    // its own when asked about the machine.
                    cwd: query
                        .root
                        .as_ref()
                        .map_or_else(|| "C:/fixture".to_owned(), |root| root.as_str().to_owned())
                        .into(),
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
    fn provider_refresh_is_absent_from_authenticated_session_streams() {
        for method in [
            RuntimeMethod::ProvidersList,
            RuntimeMethod::ProvidersWatch,
            RuntimeMethod::ProvidersGetCapabilities,
            RuntimeMethod::ProvidersListModels,
            RuntimeMethod::ProvidersListNativeSessions,
            RuntimeMethod::SessionsStart,
            RuntimeMethod::SessionsAdoptNative,
            RuntimeMethod::SessionsResume,
        ] {
            assert!(
                method_needs_provider_refresh(method),
                "missing refresh for {method:?}"
            );
        }
        for method in [
            RuntimeMethod::Initialize,
            RuntimeMethod::IntegrationsWatchEnrollment,
            RuntimeMethod::SessionsList,
            RuntimeMethod::SessionsWatchIndex,
            RuntimeMethod::SessionsWatchEvents,
        ] {
            assert!(
                !method_needs_provider_refresh(method),
                "unrelated request refreshes providers: {method:?}"
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
            reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                id: "provider-global-effort".into(),
                description: "Provider global effort".into(),
            }],
            why: "the provider reports a partial list".into(),
        });
        let RuntimeModelCatalog::Partial {
            aliases,
            models,
            reasoning_efforts,
            why,
        } = catalogue
        else {
            panic!("coverage must remain partial");
        };
        assert_eq!(aliases, ["provider-alias"]);
        assert_eq!(
            reasoning_efforts
                .first()
                .expect("one mapped global reasoning effort")
                .id,
            "provider-global-effort"
        );
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

    #[test]
    fn reasoning_effort_validation_uses_the_current_provider_catalogue() {
        let catalogue = runtrol_provider::ModelCatalog::Partial {
            aliases: vec!["provider-alias".into()],
            models: vec![runtrol_provider::ModelChoice {
                id: "provider-model".into(),
                display_name: "Provider Model".into(),
                description: "Provider description".into(),
                is_default: true,
                reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                    id: "model-effort".into(),
                    description: "Model effort".into(),
                }],
            }],
            reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                id: "global-effort".into(),
                description: "Global effort".into(),
            }],
            why: "the provider reports a partial list".into(),
        };

        assert!(reasoning_effort_is_current(
            &catalogue,
            Some("provider-model"),
            "model-effort"
        ));
        assert!(!reasoning_effort_is_current(
            &catalogue,
            Some("provider-model"),
            "global-effort"
        ));
        assert!(reasoning_effort_is_current(
            &catalogue,
            Some("provider-alias"),
            "global-effort"
        ));
        assert!(!reasoning_effort_is_current(
            &catalogue,
            Some("provider-alias"),
            "missing-effort"
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
        let locator_path = directory.join("runtime.locator.json");
        let instance = "rtm_0123456789abcdef0123456789abcdef";
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
        let identity = crate::generations::GenerationIdentity::of_this_executable()
            .expect("the test runner measures itself");
        let endpoint = composed
            .home
            .paths()
            .generation_runtime_endpoint(identity.tag())
            .expect("generation Runtime endpoint")
            .address()
            .to_owned();
        let mut listener = runtrol_ipc::transport::Listener::bind_owner_only(&endpoint)
            .await
            .expect("bind owner-only Runtime endpoint");
        let published = crate::generations::PublishedGeneration::publish(
            composed.home.paths(),
            instance,
            &identity,
            &endpoint,
            "control-endpoint-of-this-test",
        )
        .await
        .expect("publish owner-only locator");
        let fixture_provider =
            runtrol_provider::ProviderId::parse("native-fixture").expect("valid provider");
        let sessions = Arc::new(
            crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
                fixture_provider,
                "fixture-native-one",
                &resume_project,
            ),
        );
        let (provider_updates, _provider_updates_receiver) =
            watch::channel(Arc::new(crate::runtime_inventory::providers(&composed)));
        let (publishing, watching) = watch::channel(sessions.clone());
        let (usage_publishing, usage_watching) =
            watch::channel(Arc::new(ProviderUsageList::default()));
        let (runtime_asking, runtime_asked) = mpsc::channel(1);
        let (runtime_returning, runtime_returned) = mpsc::unbounded_channel();
        let owning = tokio::spawn(crate::runtime_control::fixture_runtime_owner(
            Arc::clone(&composed),
            runtime_asked,
            runtime_returned,
        ));
        let discovering = Arc::new(crate::serve::DiscoveryGates::new(&composed.registry));
        let native_cursors =
            Arc::new(NativeCursorCodec::new().expect("create native catalogue cursor authority"));
        let serving = tokio::spawn({
            let composed = Arc::clone(&composed);
            let discovering = Arc::clone(&discovering);
            let native_cursors = Arc::clone(&native_cursors);
            let provider_updates = provider_updates.clone();
            async move {
                let mut connections = tokio::task::JoinSet::new();
                for _ in 0..6 {
                    let connection = listener.accept().await.expect("accept public client");
                    connections.spawn(serve_connection(
                        connection,
                        instance.to_owned(),
                        Arc::clone(&composed),
                        Arc::clone(&discovering),
                        Arc::clone(&native_cursors),
                        provider_updates.clone(),
                        watching.clone(),
                        usage_watching.clone(),
                        runtime_asking.clone(),
                        runtime_returning.clone(),
                    ));
                }
                while let Some(joined) = connections.join_next().await {
                    joined.expect("public connection task");
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
                    AppScope::SessionStop,
                    AppScope::SessionDelete,
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
                        AppScope::SessionStop.as_str().into(),
                        AppScope::SessionDelete.as_str().into(),
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
                    .with_credentials(credentials.clone()),
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
                root: Some(project.to_string()),
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
            model: None,
            reasoning_effort: None,
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
                model: None,
                reasoning_effort: None,
            })
            .await
            .expect_err("stale resume generation is rejected");
        assert!(matches!(
            stale_resume,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::SessionConflict
        ));
        let cool = runtrol_runtime_protocol::CoolSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            session_id: resumed.session.session_id.clone(),
            expected_session_generation: resumed.session.session_generation,
            lease_id: resumed.control.lease_id.clone(),
            lease_generation: resumed.control.lease_generation,
        };
        approved
            .sessions()
            .cool(&cool)
            .await
            .expect("cool the exact idle resumed session");
        approved
            .sessions()
            .cool(&cool)
            .await
            .expect("replay the completed cool mutation");
        let cooled = approved
            .sessions()
            .get(resumed.session.session_id.clone())
            .await
            .expect("observe the cold pointer before requesting removal");
        let forget = runtrol_runtime_protocol::ForgetSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            session_id: resumed.session.session_id.clone(),
            expected_session_generation: cooled.session_generation,
        };
        let presence = approved
            .sessions()
            .forget(&forget)
            .await
            .expect_err("forget requires the exact local approval action");
        assert!(
            matches!(
                &presence,
                runtrol_runtime_client::ClientError::Runtime(error)
                    if error.code == RuntimeErrorKind::PresenceRequired
                    && error.operator_action.as_deref()
                        == Some("reviewRuntimeRequestsInRuntrolStudio")
                        && error.correlation_id.starts_with("fgt_")
            ),
            "unexpected forget admission: {presence:?}"
        );
        let Ok(pending_forgets) = composed.integration_admin.forget_requests(&composed).await
        else {
            panic!("list exact forget for local presentation");
        };
        let pending_forget = pending_forgets.first().expect("one pending forget");
        assert_eq!(
            pending_forget.session_id.as_ref(),
            stored_session.to_string()
        );
        assert_eq!(
            pending_forget.integration_id.as_ref(),
            "int_09090909090909090909090909090909"
        );
        assert!(
            composed
                .integration_admin
                .confirm_forget(&pending_forget.confirmation_id)
                .await
                .is_ok(),
            "confirm exact forget through local administration"
        );
        let Ok(remaining_forgets) = composed.integration_admin.forget_requests(&composed).await
        else {
            panic!("list confirmed forget state");
        };
        assert!(remaining_forgets.is_empty());
        approved
            .sessions()
            .forget(&forget)
            .await
            .expect("retry exact forget after local close confirmation");
        approved
            .sessions()
            .forget(&forget)
            .await
            .expect("replay completed forget mutation");
        let denied_root = approved
            .providers()
            .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                root: Some(directory.to_string_lossy().into_owned()),
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
                root: Some(project.to_string()),
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
                root: Some(project.to_string()),
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
            reasoning_effort: None,
            permission: None,
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
        let shared_start = runtrol_runtime_protocol::StartSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            workspace: start_project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Shared,
            model: None,
            reasoning_effort: None,
            permission: None,
        };
        let shared = approved
            .sessions()
            .start(&shared_start)
            .await
            .expect_err("public shared writer admission requires local presence");
        assert!(
            matches!(
                &shared,
                runtrol_runtime_client::ClientError::Runtime(error)
                    if error.code == RuntimeErrorKind::PresenceRequired
                    && error.operator_action.as_deref()
                        == Some("reviewRuntimeRequestsInRuntrolStudio")
                    && error.correlation_id.starts_with("sho_")
            ),
            "unexpected shared open admission: {shared:?}"
        );
        let Ok(pending_opens) = composed
            .integration_admin
            .shared_open_requests(&composed)
            .await
        else {
            panic!("list exact shared open for local presentation");
        };
        let pending_open = pending_opens.first().expect("one pending shared open");
        assert_eq!(pending_open.workspace.as_ref(), start_project.to_string());
        assert_eq!(pending_open.provider_id.as_ref(), "native-fixture");
        assert_eq!(pending_open.operation.as_ref(), "sessions/start");
        assert!(
            composed
                .integration_admin
                .confirm_shared_open(&pending_open.confirmation_id)
                .await
                .is_ok(),
            "confirm exact shared open through local administration"
        );
        let shared_opened = approved
            .sessions()
            .start(&shared_start)
            .await
            .expect("retry exact shared start after local confirmation");
        assert_ne!(shared_opened.session.session_id, started.session.session_id);

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

        let replacement = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([8; 32]);
        let rotation_request = runtrol_runtime_protocol::MutationRequestId::now();
        let rotation_presence = approved
            .integrations()
            .rotate_key(rotation_request.clone(), grant.key_generation, &replacement)
            .await
            .expect_err("integration key rotation requires exact local confirmation");
        assert!(matches!(
            rotation_presence,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::PresenceRequired
                && error.operator_action.as_deref()
                    == Some("reviewRuntimeRequestsInRuntrolStudio")
                && error.correlation_id.starts_with("rot_")
        ));
        assert_eq!(
            composed
                .store
                .get_integration(integration)
                .expect("read integration before local key confirmation")
                .expect("integration exists")
                .key_generation,
            1
        );
        let Ok(pending_rotations) = composed
            .integration_admin
            .key_rotation_requests(&composed)
            .await
        else {
            panic!("list exact key rotation for local presentation");
        };
        let pending_rotation = pending_rotations.first().expect("one pending key rotation");
        assert_eq!(pending_rotation.current_key_generation, 1);
        assert_eq!(
            pending_rotation.integration_id.as_ref(),
            "int_09090909090909090909090909090909"
        );
        assert!(
            composed
                .integration_admin
                .confirm_key_rotation(&pending_rotation.confirmation_id)
                .await
                .is_ok(),
            "confirm exact key rotation through local administration"
        );
        let rotated_credentials = approved
            .integrations()
            .rotate_key(rotation_request.clone(), grant.key_generation, &replacement)
            .await
            .expect("retry exact key rotation after local confirmation");
        assert_eq!(rotated_credentials.grant().key_generation, 2);
        drop(approved);
        let old_key = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_credentials(credentials.clone()),
            )
            .await;
        assert!(matches!(
            old_key,
            Err(runtrol_runtime_client::ClientError::Runtime(error))
                if error.code == RuntimeErrorKind::Unauthenticated
        ));
        let mut rotated = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_credentials(rotated_credentials.clone()),
            )
            .await
            .expect("the replacement key authenticates at its new generation");
        let replayed_credentials = rotated
            .integrations()
            .rotate_key(rotation_request, grant.key_generation, &replacement)
            .await
            .expect("the replacement key can replay the completed rotation");
        assert_eq!(replayed_credentials.grant(), rotated_credentials.grant());
        drop(rotated);
        let mut watching_client = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_credentials(rotated_credentials.clone()),
            )
            .await
            .expect("connect a dedicated index watcher");
        {
            let mut provider_watching_client = locator
                .connect(
                    runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                        .with_credentials(rotated_credentials),
                )
                .await
                .expect("connect a dedicated provider watcher");
            let mut provider_client = provider_watching_client.providers();
            let mut provider_watch = provider_client
                .watch()
                .await
                .expect("watch the structural provider inventory");
            assert_eq!(provider_watch.started().snapshot.providers.len(), 1);
            // The account usage rides the same subscription: once at the start, then on every change,
            // so a surface never asks `providers/usage` on a clock.
            let first = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
                .await
                .expect("the initial usage snapshot arrives without polling")
                .expect("typed usage notification");
            assert!(matches!(
                first,
                runtrol_runtime_client::ProviderNotification::UsageChanged(
                    runtrol_runtime_protocol::ProvidersUsageChangedNotification { .. }
                )
            ));
            let initial_providers = provider_watch.started().snapshot.clone();
            let mut changed_providers = initial_providers.clone();
            changed_providers
                .providers
                .first_mut()
                .expect("one fixture provider")
                .display_name = "Changed fixture provider".to_owned();
            provider_updates.send_replace(Arc::new(changed_providers));
            let changed = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
                .await
                .expect("provider changes arrive without polling")
                .expect("typed provider change notification");
            assert!(matches!(
                changed,
                runtrol_runtime_client::ProviderNotification::Changed(
                    runtrol_runtime_protocol::ProvidersChangedNotification { .. }
                )
            ));
            usage_publishing.send_replace(Arc::new(ProviderUsageList {
                providers: vec![runtrol_runtime_protocol::ProviderUsageGauge {
                    provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                    reached: false,
                    windows: Vec::new(),
                    cost: None,
                    tokens_today: Some(1234),
                    at_ms: 1,
                }],
            }));
            let moved = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
                .await
                .expect("a usage change arrives without polling")
                .expect("typed usage notification");
            match moved {
                runtrol_runtime_client::ProviderNotification::UsageChanged(notification) => {
                    assert_eq!(
                        notification
                            .snapshot
                            .providers
                            .first()
                            .and_then(|g| g.tokens_today),
                        Some(1234)
                    );
                }
                other => panic!("expected the usage change, got {other:?}"),
            }

            {
                let mut session_client = watching_client.sessions();
                let mut index = session_client
                    .watch_index()
                    .await
                    .expect("watch the authorized session index");
                assert_eq!(index.started().snapshot.sessions.len(), 1);
                assert!(
                    composed
                        .store
                        .revoke_integration(integration, runtrol_provider::WallMs::now())
                        .expect("revoke integration")
                );
                publishing.send_replace(sessions);
                provider_updates.send_replace(Arc::new(initial_providers));
                let ended = tokio::time::timeout(Duration::from_secs(2), index.next())
                    .await
                    .expect("revocation retires the index watch without polling")
                    .expect("typed index end notification");
                assert!(matches!(
                    ended,
                    runtrol_runtime_client::SessionIndexNotification::Ended(
                        runtrol_runtime_protocol::SessionIndexEndedNotification {
                            reason:
                                runtrol_runtime_protocol::SessionIndexEndReason::IntegrationRevoked,
                            ..
                        }
                    )
                ));
            }

            let provider_ended = async {
                for _ in 0..3 {
                    let notification = provider_watch
                        .next()
                        .await
                        .expect("typed provider watch notification");
                    if let runtrol_runtime_client::ProviderNotification::Ended(ended) = notification
                    {
                        return ended;
                    }
                }
                panic!("provider watch did not end after revocation");
            };
            let provider_ended = tokio::time::timeout(Duration::from_secs(2), provider_ended)
                .await
                .expect("revocation retires the provider watch without polling");
            assert_eq!(
                provider_ended.reason,
                runtrol_runtime_protocol::ProviderWatchEndReason::IntegrationRevoked
            );
        }
        drop(watching_client);
        drop(publishing);
        serving.await.expect("public server task finishes");
        owning.await.expect("Runtime owner task finishes");
        drop(published);
        drop(composed);
        drop(std::fs::remove_dir_all(directory));
    }
}
