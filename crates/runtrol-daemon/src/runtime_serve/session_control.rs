//! Managed session opening and connection-bound control requests.

use std::time::Duration;

use runtrol_core::{ApprovalAuthority, WorkspaceClaim};
use runtrol_provider::{CloseMode, Disposition, OpenIntent, WorkspaceAccess};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, ControlLeaseParams,
    CoolSessionParams, JsonRpcId, ListPendingApprovalsParams, MAX_MODEL_SELECTION_BYTES,
    MAX_NATIVE_ADOPTION_TOKEN_BYTES, RespondApprovalParams, ResumeSessionParams, RuntimeErrorKind,
    RuntimeMethod, RuntimeSessionId, SessionWorkspaceAccess, SetModeParams, SetModelParams,
    StartSessionParams, SubmitBlocksParams, SubmitInputParams, WatchEventsParams,
};
use tokio::sync::{mpsc, oneshot};

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_control::{
    ApprovalScopes, RuntimeAsked, RuntimeControlFailure, RuntimeControlReply,
    RuntimeControlRequest, RuntimeOpenCompletion, RuntimeOpenGuard, RuntimeOpenRequest,
    RuntimeReturned,
};
use crate::runtime_inventory::{
    RuntimeInventoryFailure, RuntimeSessionCatalogue, authorized_roots, authorized_workspace,
};
use crate::runtime_native_sessions::NativeCursorCodec;

use super::authority::{authorized, required_scope};
use super::connection_state::PublicState;
use super::response::{
    Answer, confirmation_failure, control_failure, inventory_failure, random_subscription_id,
    runtime_owner_stopped,
};

#[expect(
    clippy::too_many_arguments,
    reason = "the public open boundary keeps closed parsing, live authority, workspace identity, and owner reservation ordering together"
)]
pub(super) async fn open_session(
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
    let authority = match authorized(state, composed, required_scope(method)) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let request = match build_open_request(method, params, &authority, sessions, composed).await {
        Ok(request) => request,
        Err(OpenAdmissionFailure::Control(failure)) => return control_failure(id, &failure),
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
        RuntimeControlReply::Failed(failure) => control_failure(id, &failure),
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
        None => return control_failure(id, &RuntimeControlFailure::outcome_unknown()),
    };
    let authority = match authorized(state, composed, required_scope(method)) {
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
        None => return control_failure(id, &RuntimeControlFailure::outcome_unknown()),
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
        None => return control_failure(id, &RuntimeControlFailure::outcome_unknown()),
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
            selected_model.is_none_or(|model| catalogue.contains_model(model))
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
        None => return control_failure(id, &RuntimeControlFailure::outcome_unknown()),
    };
    let opened = tokio::time::timeout(
        Duration::from_millis(crate::serve::MODEL_PREPARATION_BUDGET_MS),
        prepared.driver.open(intent.clone()),
    )
    .await;
    match opened {
        Ok(Ok(agent)) => {
            let still_authorized = match authorized(state, composed, required_scope(method)) {
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

pub(super) fn reasoning_effort_is_current(
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
        return control_failure(id, &RuntimeControlFailure::outcome_unknown());
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
        return control_failure(id, &RuntimeControlFailure::outcome_unknown());
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
        return control_failure(id, &RuntimeControlFailure::outcome_unknown());
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
        RuntimeOpenCompletion::Answer(Err(failure)) => control_failure(id, &failure),
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
                Ok(Err(failure)) => control_failure(id, &failure),
                Err(_) => runtime_owner_stopped(id),
            }
        }
    }
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

#[expect(
    clippy::too_many_arguments,
    reason = "one public session boundary keeps authorization, catalogue resolution, owner handoff, and response identity visible together"
)]
pub(super) async fn session_operation(
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
    let authority = match authorized(state, composed, required_scope(method)) {
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
        return runtime_owner_stopped(id);
    }
    let Ok(reply) = hearing.await else {
        return runtime_owner_stopped(id);
    };
    super::response::runtime_control_answer(id, reply, returning).await
}

/// Whether this provider accepts a runtrol switch to the named mode.
///
/// The manifest's `switchable` list is the boundary for a CLI whose vocabulary cannot be discovered, and it
/// deliberately omits the modes that remove safety prompts, so those are unreachable through this method for
/// every caller. An empty list means the protocol announces modes per session, and the driver itself gates on
/// that announcement (measured: one agent confirms unannounced switches with an empty success, which is why
/// somebody must gate). A session whose provider cannot be identified is refused rather than relayed.
pub(super) fn mode_within_provider_vocabulary(
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
pub(super) fn mode_within_manifest_vocabulary(
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

#[expect(
    clippy::too_many_lines,
    reason = "every public method is named so a new one cannot fall through to the session lane by omission"
)]
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
        | RuntimeMethod::ProvidersNativeActivity
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
        | RuntimeMethod::TerminalsSetDialogue
        | RuntimeMethod::TerminalsIndexChanged
        | RuntimeMethod::TerminalsIndexEnded
        | RuntimeMethod::TerminalsOutput
        | RuntimeMethod::TerminalsLagged
        | RuntimeMethod::TerminalsExited
        | RuntimeMethod::WindowsRegister
        | RuntimeMethod::WindowsUpdate
        | RuntimeMethod::WindowsList
        | RuntimeMethod::WindowsWatchIndex
        | RuntimeMethod::WindowsMirrorOpen
        | RuntimeMethod::WindowsMirrorOutput
        | RuntimeMethod::WindowsMirrorEnd
        | RuntimeMethod::WindowsReveal
        | RuntimeMethod::WindowsWatchReveals
        | RuntimeMethod::WindowsIndexChanged
        | RuntimeMethod::WindowsIndexEnded
        | RuntimeMethod::WindowsRevealRequested
        | RuntimeMethod::WindowsRevealsEnded
        | RuntimeMethod::SessionsEvent
        | RuntimeMethod::SessionsLagged
        | RuntimeMethod::SessionsIndexChanged
        | RuntimeMethod::SessionsIndexEnded
        | RuntimeMethod::ProvidersUsage
        | RuntimeMethod::ProvidersFocusNative
        | RuntimeMethod::PanicStop => Err("the method is not a session operation"),
    }
}
