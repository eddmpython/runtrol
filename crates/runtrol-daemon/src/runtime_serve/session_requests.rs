//! Managed session queries and provider-native session mutations.

use std::time::Duration;

use runtrol_runtime_protocol::{
    AppScope, ArchiveNativeSessionParams, DeleteNativeSessionParams, ForgetSessionParams,
    GetSessionParams, JsonRpcId, RuntimeErrorKind, RuntimeMethod, WatchSessionIndexParams,
    WatchSessionIndexResult,
};
use tokio::sync::{mpsc, oneshot};

use crate::Composed;
use crate::runtime_control::{RuntimeAsked, RuntimeControlRequest, RuntimeReturned};
use crate::runtime_inventory::{
    RuntimeInventoryFailure, RuntimeSessionCatalogue, authorized_workspace,
};

use super::authority::authorized;
use super::connection_state::PublicState;
use super::response::{
    Answer, EmptyParams, confirmation_failure, control_failure, inventory_failure,
    random_subscription_id, runtime_control_answer, runtime_owner_stopped,
};

pub(super) fn sessions_list(
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
    match authorized(state, composed, Some(AppScope::SessionList)) {
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

pub(super) fn sessions_watch_index(
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
    let authority = match authorized(state, composed, Some(AppScope::SessionList)) {
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

pub(super) fn sessions_get(
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
    match authorized(state, composed, Some(AppScope::SessionList)) {
        Ok(authority) => match sessions.authorized_descriptor(authority, &params.session_id) {
            Ok(descriptor) => Answer::success(id, &descriptor),
            Err(failure) => inventory_failure(id, failure),
        },
        Err(failure) => Answer::failure(id, failure),
    }
}

pub(super) async fn forget_session(
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
    let authority = match authorized(state, composed, Some(AppScope::SessionDelete)) {
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
pub(super) enum NativeSessionMutation {
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

pub(super) struct NativeSessionMutationParams {
    provider_id: runtrol_runtime_protocol::ProviderId,
    pub(super) native_session_id: String,
    workspace: String,
}

pub(super) fn parse_native_session_mutation(
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
pub(super) async fn mutate_native_session(
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
    let authority = match authorized(state, composed, Some(AppScope::SessionDelete)) {
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
    if let Some(refusal) = refuse_live_native_claim(&id, composed, provider, &native, &workspace) {
        return refusal;
    }
    if let Some(refusal) = refuse_supervised(id.clone(), sessions, &authority, provider, &native) {
        return refusal;
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
    // The asked identity survives the provider's answer: a deletion then forgets Runtrol's pointers to it,
    // and the folder is named in the record of what was removed.
    let asked = native.clone();
    let removed_from = workspace.clone();
    let origin = crate::native_deletions::MutationOrigin {
        integration: &authority,
        workspace: &removed_from,
    };
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
        Ok(Ok(())) => mutated_answer(id, mutation, composed, provider, &native, &origin),
        // The provider's own answer, by kind: unsupported stays unsupported, a refusal stays a refusal.
        Ok(Err(error)) => control_failure(id, &crate::runtime_control::provider_failure(&error)),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::RuntimeUnavailable,
            mutation.timeout_message(),
        ),
    }
}

/// A provider process or unresolved launch owns the mutation boundary until it stops.
pub(super) fn refuse_live_native_claim(
    id: &JsonRpcId,
    composed: &Composed,
    provider: runtrol_provider::ProviderId,
    native: &runtrol_provider::NativeSessionId,
    workspace: &runtrol_provider::AbsPath,
) -> Option<Answer> {
    match composed.native_claims.blocks_native_mutation(
        provider.as_str(),
        native.as_str(),
        workspace.as_str(),
    ) {
        Ok(false) => None,
        Ok(true) => Some(Answer::plain(
            id.clone(),
            RuntimeErrorKind::NativeConversationBusy,
            "the provider-native conversation has a live process claim; stop its original session before mutating it",
        )),
        Err(_) => Some(Answer::plain(
            id.clone(),
            RuntimeErrorKind::RuntimeUnavailable,
            "the native live-claim registry is unavailable",
        )),
    }
}

/// Refuse to change a conversation Runtime is supervising: the process would answer to a store that moved
/// under it. Named so the mutation path reads as its three steps rather than carrying the guard inline.
pub(super) fn refuse_supervised(
    id: JsonRpcId,
    sessions: &RuntimeSessionCatalogue,
    authority: &crate::runtime_auth::AuthorizedIntegration,
    provider: runtrol_provider::ProviderId,
    native: &runtrol_provider::NativeSessionId,
) -> Option<Answer> {
    match sessions.managed_as(authority, provider, native) {
        Ok(None) => None,
        Ok(Some(_)) => Some(Answer::plain(
            id,
            RuntimeErrorKind::SessionConflict,
            "Runtime supervises this conversation; forget the supervised session first",
        )),
        Err(failure) => Some(inventory_failure(id, failure)),
    }
}

/// Name the conversations of one provider with a model answering in them right now.
///
/// The panel asks this often, so it must stay the cheap question: the driver answers from whatever its own
/// service already publishes about its running processes, and opens no conversation. It is the only way
/// Runtrol can say that a conversation it did not start is running, which is most of them for a person who
/// also uses their CLI in a terminal (operator, 2026-08-28).
///
/// No folder filter and no per-row authorisation: an identity the caller was already shown by the catalogue,
/// answered on the owner-only local endpoint, adds nothing the caller does not have. The same argument the
/// machine-wide catalogue makes for itself (`docs/runtimeProtocol.md`).
/// Mirror the sessions a person started outside Runtrol so they become one session, streamed to every window.
///
/// A session started anywhere is still one session. A live terminal interface the daemon does not already host
/// as its own PTY child is mirrored: a helper joins its console (Windows) so every window sees the same screen
/// and can type into it, and the row becomes a hosted one that a click attaches to instead of resuming a copy.
/// A piped or SDK child has no screen to join, so only an interactive process is mirrored. Failure to mirror
/// one process leaves it observed-external, the honest fallback, and never fails the activity answer.
/// Ask one provider what it has open, behind its own lane, its freshness cache and one bounded wait.
///
/// `Err(())` is a provider that could not be prepared or would not answer; the outer `Err` is the wait running
/// out. Both leave the last answer standing rather than replacing it with an empty one.
fn mutated_answer(
    id: JsonRpcId,
    mutation: NativeSessionMutation,
    composed: &Composed,
    provider: runtrol_provider::ProviderId,
    native: &runtrol_provider::NativeSessionId,
    origin: &crate::native_deletions::MutationOrigin<'_>,
) -> Answer {
    if !matches!(mutation, NativeSessionMutation::Delete) {
        return Answer::success(id, &serde_json::json!({}));
    }
    crate::native_deletions::record(composed, origin, provider, native);
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
pub(super) fn forget_pointers_of(
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
