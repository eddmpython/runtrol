//! Separate public Runtime listener with challenge-bound integration authority.

use std::collections::BTreeSet;
use std::sync::Arc;

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    AppScope, ErrorResponse, FINALIZED_REVISIONS, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, MAX_REVISION_OFFERS,
    RequestEnrollmentParams, RuntimeCapabilities, RuntimeError, RuntimeErrorKind, RuntimeInstance,
    RuntimeLimits, RuntimeMethod, SuccessResponse, WatchEnrollmentParams, negotiate,
};
use runtrol_store::EnrollmentKey;
use runtrol_store::IntegrationAuditOutcome;
use serde::Serialize;
use tokio::sync::watch;

use crate::Composed;
use crate::runtime_auth::{
    AuthorizationFailure, AuthorizedIntegration, ClientContext, authenticate, challenge,
    enrollment_decision, refresh, request_enrollment,
};
use crate::runtime_inventory::{RuntimeInventoryFailure, RuntimeSessionCatalogue};

/// Serve one public connection until it closes or violates the public frame contract.
pub(crate) async fn serve_connection(
    mut connection: Connection,
    instance_id: String,
    composed: Arc<Composed>,
    sessions: watch::Receiver<Arc<RuntimeSessionCatalogue>>,
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
        let answered = answer(
            &mut state,
            &instance_id,
            &composed,
            &sessions.borrow(),
            request,
        );
        if send_response(&mut connection, &answered.response)
            .await
            .is_err()
            || answered.close
        {
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
}

impl Answer {
    fn success<T: Serialize>(id: JsonRpcId, result: &T) -> Self {
        Self {
            response: success(id, result),
            close: false,
        }
    }

    fn failure(id: JsonRpcId, failure: AuthorizationFailure) -> Self {
        let close = failure.kind == RuntimeErrorKind::IntegrationRevoked;
        Self {
            response: failure_response(id, failure.kind, failure.message),
            close,
        }
    }

    fn plain(id: JsonRpcId, code: RuntimeErrorKind, message: &str) -> Self {
        Self {
            response: failure_response(id, code, message),
            close: false,
        }
    }
}

fn answer(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
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
        sessions,
        method,
        id,
        request.params,
    );
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

fn dispatch_public(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    sessions: &RuntimeSessionCatalogue,
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
            RuntimeMethod::SessionsList => sessions_list(state, composed, sessions, id, params),
            RuntimeMethod::Initialized | RuntimeMethod::Challenge => Answer::plain(
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
        RuntimeMethod::ProvidersList => Some(AppScope::ProviderRead),
        RuntimeMethod::SessionsList => Some(AppScope::SessionList),
        RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
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
        },
        Err(failure) => Answer::failure(id, failure),
    }
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
                runtrol_drivers::builtin(),
            )
            .expect("compose test Runtime"),
        );
        let sessions = Arc::new(crate::runtime_inventory::RuntimeSessionCatalogue::unavailable());
        let (_publishing, watching) = watch::channel(sessions);
        let serving = tokio::spawn({
            let composed = Arc::clone(&composed);
            async move {
                for _ in 0..2 {
                    let connection = listener.accept().await.expect("accept public client");
                    serve_connection(
                        connection,
                        instance.to_owned(),
                        Arc::clone(&composed),
                        watching.clone(),
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
                vec![AppScope::ProviderRead],
                Vec::new(),
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
                    scopes: vec![AppScope::ProviderRead.as_str().into()],
                    roots: Vec::new(),
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
        drop(published);
        drop(composed);
        drop(std::fs::remove_dir_all(directory));
    }
}
