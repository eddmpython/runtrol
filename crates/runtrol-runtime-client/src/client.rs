//! Typed initialization, enrollment, and read-only Runtime operation groups.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, ArchiveNativeSessionParams,
    CHALLENGE_LIFETIME_MS, ClientCapabilities, ClientInfo, ControlLease, ControlLeaseParams,
    CoolSessionParams, DeleteNativeSessionParams, EnrollmentDecision, EnrollmentManifest,
    EnrollmentReceipt, ErrorResponse, EventCursor, FINALIZED_REVISIONS, ForgetSessionParams,
    GetProviderCapabilitiesParams, GetSessionParams, InitializeParams, InitializeResult,
    IntegrationAuthentication, IntegrationGrant, JsonRpcId, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LaggedNotification, ListModelsParams, ListNativeSessionsParams,
    ListPendingApprovalsParams, ManagedSessionList, MutationRequestId, NativeSessionCatalogue,
    PendingApprovalList, PendingEnrollmentId, ProviderId, ProviderList, ProviderUsageList,
    ProviderWatchEndedNotification, ProvidersChangedNotification, RequestEnrollmentParams,
    RespondApprovalParams, ResumeSessionParams, RotateIntegrationKeyParams, RuntimeError,
    RuntimeErrorKind, RuntimeEventNotification, RuntimeMethod, RuntimeModelCatalog,
    RuntimeProviderCapabilities, RuntimeSessionId, ServerChallenge, SessionDescriptor,
    SessionIndexChangedNotification, SessionIndexEndedNotification, SessionOpenResult,
    SetModeParams, SetModelParams, StartSessionParams, SubmitBlocksParams, SubmitInputParams,
    SuccessResponse, WatchEnrollmentParams, WatchEventsParams, WatchEventsResult,
    WatchProvidersParams, WatchProvidersResult, WatchSessionIndexParams, WatchSessionIndexResult,
    enrollment_signing_payload, initialization_signing_payload, key_rotation_signing_payload,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::ClientError;
use crate::connection::Connection;
use crate::identity::{IntegrationCredentials, IntegrationIdentity};
use crate::locator::{LocatorError, LocatorState, RuntimeLocator, ValidatedLocator};

const CHALLENGE_CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;

/// Safe client metadata and optional consumer-owned integration identity.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    name: String,
    version: String,
    capabilities: ClientCapabilities,
    identity: Option<ClientIdentity>,
}

#[derive(Clone, Debug)]
enum ClientIdentity {
    Enrolling(IntegrationIdentity),
    Approved(IntegrationCredentials),
}

impl ClientOptions {
    /// Describe a consumer without claiming authorization identity.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            capabilities: ClientCapabilities::default(),
            identity: None,
        }
    }

    /// Attach a new or previously persisted identity for enrollment.
    #[must_use]
    pub fn with_identity(mut self, identity: IntegrationIdentity) -> Self {
        self.identity = Some(ClientIdentity::Enrolling(identity));
        self
    }

    /// Attach an approved identity and current generations for authenticated reconnect.
    #[must_use]
    pub fn with_credentials(mut self, credentials: IntegrationCredentials) -> Self {
        self.identity = Some(ClientIdentity::Approved(credentials));
        self
    }
}

/// Exact public proposal signed for one local enrollment decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentProposal {
    client_instance_id: String,
    manifest_digest: [u8; 32],
    requested_scopes: Vec<AppScope>,
    requested_roots: Vec<String>,
}

/// Capped reconnect admission for connection establishment only.
///
/// Runtime mutations are never retried by this policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconnectPolicy {
    initial_delay: Duration,
    maximum_delay: Duration,
    deadline: Duration,
}

impl ReconnectPolicy {
    /// Construct a bounded exponential reconnect policy.
    ///
    /// # Errors
    ///
    /// A zero duration, an initial delay above the maximum, or a maximum delay above the total deadline.
    pub fn new(
        initial_delay: Duration,
        maximum_delay: Duration,
        deadline: Duration,
    ) -> Result<Self, ClientError> {
        if initial_delay.is_zero()
            || maximum_delay.is_zero()
            || deadline.is_zero()
            || initial_delay > maximum_delay
            || maximum_delay > deadline
        {
            return Err(ClientError::Protocol(
                "reconnect delays must be nonzero and ordered within the deadline".to_owned(),
            ));
        }
        Ok(Self {
            initial_delay,
            maximum_delay,
            deadline,
        })
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            maximum_delay: Duration::from_secs(2),
            deadline: Duration::from_secs(30),
        }
    }
}

impl EnrollmentProposal {
    /// Create a closed proposal. Runtime validates bounds and canonicalizes roots during local approval.
    #[must_use]
    pub fn new(
        client_instance_id: impl Into<String>,
        manifest_digest: [u8; 32],
        requested_scopes: Vec<AppScope>,
        requested_roots: Vec<String>,
    ) -> Self {
        Self {
            client_instance_id: client_instance_id.into(),
            manifest_digest,
            requested_scopes,
            requested_roots,
        }
    }
}

impl RuntimeLocator {
    /// Connect, validate the server-first challenge, negotiate, and finish initialization.
    ///
    /// # Errors
    ///
    /// Locator validation, local transport, protocol, incompatibility, signing, or Runtime failures.
    pub async fn connect(&self, options: ClientOptions) -> Result<RuntimeClient, ClientError> {
        let LocatorState::Running(locator) = self.inspect()? else {
            return Err(ClientError::Runtime(
                runtrol_runtime_protocol::RuntimeError::plain(
                    runtrol_runtime_protocol::RuntimeErrorKind::RuntimeNotInstalled,
                    "Runtrol Runtime is not installed",
                    "local-locator",
                ),
            ));
        };
        RuntimeClient::connect(locator, options).await
    }

    /// Connect with capped exponential backoff and jitter for transient locator or transport failures.
    ///
    /// Each attempt re-reads the owner-validated locator, so a restarted Runtime may publish a replacement endpoint.
    /// Authentication, protocol, enrollment, revocation, and other non-retryable failures return immediately. This
    /// helper establishes a connection only and never retries a Runtime operation.
    ///
    /// # Errors
    ///
    /// The first non-retryable failure or the last transient failure observed at the policy deadline.
    pub async fn connect_with_retry(
        &self,
        options: ClientOptions,
        policy: ReconnectPolicy,
    ) -> Result<RuntimeClient, ClientError> {
        let deadline = tokio::time::Instant::now() + policy.deadline;
        let mut delay = policy.initial_delay;
        loop {
            match self.connect(options.clone()).await {
                Ok(client) => return Ok(client),
                Err(error) if retryable_connection_failure(&error) => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(jittered(delay).min(deadline - now)).await;
                    delay = delay.saturating_mul(2).min(policy.maximum_delay);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Open a read-only event stream that reconnects from the last cursor explicitly accepted by the consumer.
    ///
    /// The reconnect path sends only `sessions/watchEvents`. It never retries input, approval, lease, or lifecycle
    /// mutations.
    ///
    /// # Errors
    ///
    /// Connection, authentication, scope, cursor, or subscription admission failure.
    pub async fn watch_events_with_reconnect(
        &self,
        options: ClientOptions,
        params: WatchEventsParams,
        policy: ReconnectPolicy,
    ) -> Result<ReconnectingEventSubscription, ClientError> {
        ReconnectingEventSubscription::open(self.clone(), options, params, policy).await
    }

    /// Open a provider snapshot stream that replaces a lost read-only connection.
    ///
    /// # Errors
    ///
    /// Connection, authentication, scope, or subscription admission failure.
    pub async fn watch_providers_with_reconnect(
        &self,
        options: ClientOptions,
        policy: ReconnectPolicy,
    ) -> Result<ReconnectingProviderSubscription, ClientError> {
        ReconnectingProviderSubscription::open(self.clone(), options, policy).await
    }

    /// Open a managed-session snapshot stream that replaces a lost read-only connection.
    ///
    /// # Errors
    ///
    /// Connection, authentication, scope, root, or subscription admission failure.
    pub async fn watch_session_index_with_reconnect(
        &self,
        options: ClientOptions,
        policy: ReconnectPolicy,
    ) -> Result<ReconnectingSessionIndexSubscription, ClientError> {
        ReconnectingSessionIndexSubscription::open(self.clone(), options, policy).await
    }
}

fn retryable_connection_failure(error: &ClientError) -> bool {
    match error {
        ClientError::Transport { .. } | ClientError::Locator(LocatorError::Io(_)) => true,
        ClientError::Runtime(error) => error.retryable,
        ClientError::Locator(
            LocatorError::Environment { .. } | LocatorError::Malformed(_) | LocatorError::Unsafe(_),
        )
        | ClientError::Protocol(_) => false,
    }
}

fn jittered(delay: Duration) -> Duration {
    let mut random = [0_u8; 2];
    if getrandom::fill(&mut random).is_err() {
        return delay;
    }
    let basis_points = 7_500_u128 + (u128::from(u16::from_le_bytes(random)) % 5_001);
    let nanos = delay.as_nanos().saturating_mul(basis_points) / 10_000;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// One initialized public connection. It owns no Runtime or provider session.
pub struct RuntimeClient {
    connection: Connection,
    next_id: u64,
    initialized: InitializeResult,
    challenge: ServerChallenge,
    supported_revisions: Vec<runtrol_runtime_protocol::ProtocolRevision>,
    client: ClientInfo,
    capabilities: ClientCapabilities,
    identity: Option<IntegrationIdentity>,
}

impl RuntimeClient {
    async fn connect(
        locator: ValidatedLocator,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let mut connection = Connection::connect(&locator.endpoint).await?;
        let challenge = receive_challenge(&mut connection, &locator).await?;
        let supported_revisions = FINALIZED_REVISIONS.to_vec();
        let client = ClientInfo {
            name: options.name,
            version: options.version,
        };
        let capabilities = options.capabilities;
        let (identity, expected_grant) = match options.identity {
            Some(ClientIdentity::Enrolling(identity)) => (Some(identity), None),
            Some(ClientIdentity::Approved(credentials)) => {
                let (identity, grant) = credentials.into_parts();
                (Some(identity), Some(grant))
            }
            None => (None, None),
        };
        let authentication = match (&identity, &expected_grant) {
            (Some(identity), Some(grant)) => {
                let mut proof = IntegrationAuthentication {
                    integration_id: grant.integration_id.clone(),
                    key_generation: grant.key_generation,
                    grant_generation: grant.grant_generation,
                    signature: String::new(),
                };
                let payload = initialization_signing_payload(
                    &challenge,
                    &supported_revisions,
                    &client,
                    &capabilities,
                    &proof,
                )
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "initialization proof payload cannot be encoded: {error}"
                    ))
                })?;
                proof.signature = identity.sign_base64(&payload);
                Some(proof)
            }
            (Some(_) | None, None) => None,
            (None, Some(_)) => {
                return Err(ClientError::Protocol(
                    "approved integration generations have no signing identity".to_owned(),
                ));
            }
        };
        let mut next_id = 1;
        let initialized: InitializeResult = call_connection(
            &mut connection,
            &mut next_id,
            RuntimeMethod::Initialize,
            &InitializeParams {
                supported_revisions: supported_revisions.clone(),
                client: client.clone(),
                client_capabilities: capabilities.clone(),
                authentication,
            },
        )
        .await?;
        validate_initialization(&initialized, &locator, expected_grant.as_ref())?;
        notify_connection(&mut connection, RuntimeMethod::Initialized, &EmptyParams {}).await?;
        Ok(Self {
            connection,
            next_id,
            initialized,
            challenge,
            supported_revisions,
            client,
            capabilities,
            identity,
        })
    }

    /// The selected revision, Runtime instance, capabilities, limits, and authenticated grant.
    #[must_use]
    pub const fn initialization(&self) -> &InitializeResult {
        &self.initialized
    }

    /// Integration enrollment and grant operations.
    pub fn integrations(&mut self) -> IntegrationClient<'_> {
        IntegrationClient { runtime: self }
    }

    /// Provider inventory operations.
    pub fn providers(&mut self) -> ProviderClient<'_> {
        ProviderClient { runtime: self }
    }

    /// Runtime-managed session operations.
    pub fn sessions(&mut self) -> SessionClient<'_> {
        SessionClient { runtime: self }
    }

    /// Structured provider approval operations for controlled sessions.
    pub fn approvals(&mut self) -> ApprovalClient<'_> {
        ApprovalClient { runtime: self }
    }

    /// Bind an approved grant returned on this connection to its consumer-owned signing identity.
    ///
    /// # Errors
    ///
    /// The connection was created without an integration identity.
    pub fn credentials(
        &self,
        grant: IntegrationGrant,
    ) -> Result<IntegrationCredentials, ClientError> {
        let Some(identity) = &self.identity else {
            return Err(ClientError::Protocol(
                "the connection has no integration identity to persist".to_owned(),
            ));
        };
        Ok(IntegrationCredentials::new(identity.clone(), grant))
    }

    /// Stop every supervised process in the safe direction.
    ///
    /// # Errors
    ///
    /// Transport, protocol, or Runtime failure.
    pub async fn panic_stop(&mut self) -> Result<(), ClientError> {
        let _: EmptyResult = self.call(RuntimeMethod::PanicStop, &EmptyParams {}).await?;
        Ok(())
    }

    async fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        params: &P,
    ) -> Result<R, ClientError> {
        call_connection(&mut self.connection, &mut self.next_id, method, params).await
    }

    async fn call_mutation<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        request_id: &MutationRequestId,
        params: &P,
    ) -> Result<R, ClientError> {
        match self.call(method, params).await {
            Err(ClientError::Transport { .. }) => Err(ClientError::Runtime(RuntimeError::plain(
                RuntimeErrorKind::OutcomeUnknown,
                "Runtime connection ended while the mutation outcome was unresolved",
                request_id.as_str(),
            ))),
            result => result,
        }
    }
}

async fn receive_challenge(
    connection: &mut Connection,
    locator: &ValidatedLocator,
) -> Result<ServerChallenge, ClientError> {
    let payload = connection.receive().await?;
    let notification: JsonRpcNotification = serde_json::from_slice(&payload).map_err(|error| {
        ClientError::Protocol(format!(
            "the first Runtime frame is not a JSON-RPC notification: {error}"
        ))
    })?;
    if notification.jsonrpc != "2.0"
        || notification.method.parse::<RuntimeMethod>() != Ok(RuntimeMethod::Challenge)
    {
        return Err(ClientError::Protocol(
            "the first Runtime frame is not the required challenge".to_owned(),
        ));
    }
    let challenge: ServerChallenge =
        serde_json::from_value(notification.params).map_err(|error| {
            ClientError::Protocol(format!(
                "the Runtime challenge has the wrong shape: {error}"
            ))
        })?;
    validate_challenge(&challenge, locator)?;
    Ok(challenge)
}

fn validate_challenge(
    challenge: &ServerChallenge,
    locator: &ValidatedLocator,
) -> Result<(), ClientError> {
    if challenge.instance_id != locator.instance_id {
        return Err(ClientError::Protocol(
            "the challenge Runtime instance does not match the locator".to_owned(),
        ));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ClientError::Protocol(format!("system time precedes Unix epoch: {error}"))
        })?;
    let now_ms = u64::try_from(now.as_millis()).map_err(|_| {
        ClientError::Protocol("system time does not fit Runtime milliseconds".to_owned())
    })?;
    validate_challenge_at(challenge, now_ms)
}

fn validate_challenge_at(challenge: &ServerChallenge, now_ms: u64) -> Result<(), ClientError> {
    if challenge.expires_at_ms <= now_ms {
        return Err(ClientError::Protocol(
            "the Runtime challenge is already expired".to_owned(),
        ));
    }
    if challenge.expires_at_ms
        > now_ms
            .saturating_add(CHALLENGE_LIFETIME_MS)
            .saturating_add(CHALLENGE_CLOCK_SKEW_TOLERANCE_MS)
    {
        return Err(ClientError::Protocol(
            "the Runtime challenge exceeds the public lifetime and clock-skew bound".to_owned(),
        ));
    }
    let nonce = match Base64UrlUnpadded::decode_vec(&challenge.nonce) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(ClientError::Protocol(format!(
                "the Runtime challenge nonce is malformed: {error}"
            )));
        }
    };
    if nonce.len() != 32
        || challenge.nonce_id.len() != 38
        || !challenge.nonce_id.starts_with("nonce_")
        || !challenge
            .nonce_id
            .bytes()
            .skip(6)
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ClientError::Protocol(
            "the Runtime challenge nonce is malformed".to_owned(),
        ));
    }
    Ok(())
}

fn validate_initialization(
    initialized: &InitializeResult,
    locator: &ValidatedLocator,
    expected_grant: Option<&IntegrationGrant>,
) -> Result<(), ClientError> {
    if initialized.runtime.instance_id != locator.instance_id {
        return Err(ClientError::Protocol(
            "the Runtime instance does not match the locator".to_owned(),
        ));
    }
    if initialized.runtime.version != locator.runtime_version {
        return Err(ClientError::Protocol(
            "the Runtime version does not match the locator".to_owned(),
        ));
    }
    if !FINALIZED_REVISIONS.contains(&initialized.selected_revision) {
        return Err(ClientError::Protocol(
            "the Runtime selected a revision the client did not offer".to_owned(),
        ));
    }
    if !initialization_grant_matches(initialized.grant.as_ref(), expected_grant) {
        return Err(ClientError::Protocol(
            "the Runtime initialization grant does not match the authenticated credentials"
                .to_owned(),
        ));
    }
    Ok(())
}

fn initialization_grant_matches(
    current: Option<&IntegrationGrant>,
    expected: Option<&IntegrationGrant>,
) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => {
            current.integration_id == expected.integration_id
                && current.key_generation == expected.key_generation
                && current.grant_generation >= expected.grant_generation
                && (current.grant_generation != expected.grant_generation || current == expected)
        }
        (None, Some(_)) | (Some(_), None) => false,
    }
}

async fn call_connection<P: Serialize, R: DeserializeOwned>(
    connection: &mut Connection,
    next_id: &mut u64,
    method: RuntimeMethod,
    params: &P,
) -> Result<R, ClientError> {
    let id = JsonRpcId::Number(*next_id);
    *next_id = next_id.checked_add(1).ok_or_else(|| {
        ClientError::Protocol("the connection exhausted its request identifiers".to_owned())
    })?;
    let params = serde_json::to_value(params).map_err(|error| {
        ClientError::Protocol(format!("request parameters cannot be encoded: {error}"))
    })?;
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: id.clone(),
        method: method.to_string(),
        params,
    };
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| ClientError::Protocol(format!("request cannot be encoded: {error}")))?;
    connection.send(&encoded).await?;
    let response = connection.receive().await?;
    let response: JsonRpcResponse = serde_json::from_slice(&response).map_err(|error| {
        ClientError::Protocol(format!("response is not valid public JSON-RPC: {error}"))
    })?;
    match response {
        JsonRpcResponse::Success(SuccessResponse {
            jsonrpc,
            id: response_id,
            result,
        }) => {
            validate_envelope(&jsonrpc, &id, &response_id)?;
            serde_json::from_value(result).map_err(|error| {
                ClientError::Protocol(format!("method result has the wrong shape: {error}"))
            })
        }
        JsonRpcResponse::Error(ErrorResponse {
            jsonrpc,
            id: response_id,
            error,
        }) => {
            validate_envelope(&jsonrpc, &id, &response_id)?;
            Err(ClientError::Runtime(error))
        }
    }
}

async fn notify_connection<P: Serialize>(
    connection: &mut Connection,
    method: RuntimeMethod,
    params: &P,
) -> Result<(), ClientError> {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: method.to_string(),
        params: serde_json::to_value(params).map_err(|error| {
            ClientError::Protocol(format!("notification cannot be encoded: {error}"))
        })?,
    };
    let encoded = serde_json::to_vec(&notification).map_err(|error| {
        ClientError::Protocol(format!("notification cannot be encoded: {error}"))
    })?;
    connection.send(&encoded).await
}

fn validate_envelope(
    jsonrpc: &str,
    expected: &JsonRpcId,
    actual: &JsonRpcId,
) -> Result<(), ClientError> {
    if jsonrpc != "2.0" {
        return Err(ClientError::Protocol(
            "response JSON-RPC version is not 2.0".to_owned(),
        ));
    }
    if expected != actual {
        return Err(ClientError::Protocol(
            "response request identifier does not match".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct EmptyParams {}

#[derive(serde::Deserialize)]
struct EmptyResult {}

/// Typed integration enrollment methods.
pub struct IntegrationClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl IntegrationClient<'_> {
    /// Prove possession and create one bounded local enrollment request.
    ///
    /// # Errors
    ///
    /// Missing identity, protocol encoding, transport, or Runtime refusal.
    pub async fn request(
        &mut self,
        proposal: EnrollmentProposal,
    ) -> Result<EnrollmentReceipt, ClientError> {
        let Some(identity) = self.runtime.identity.as_ref() else {
            return Err(ClientError::Protocol(
                "integration enrollment requires a consumer-owned identity".to_owned(),
            ));
        };
        let manifest = EnrollmentManifest {
            client_instance_id: proposal.client_instance_id,
            public_key: identity.public_key_base64(),
            manifest_digest: Base64UrlUnpadded::encode_string(&proposal.manifest_digest),
            requested_scopes: proposal.requested_scopes,
            requested_roots: proposal.requested_roots,
        };
        let payload = enrollment_signing_payload(
            &self.runtime.challenge,
            &self.runtime.supported_revisions,
            self.runtime.initialized.selected_revision,
            &self.runtime.client,
            &self.runtime.capabilities,
            &manifest,
        )
        .map_err(|error| {
            ClientError::Protocol(format!(
                "enrollment proof payload cannot be encoded: {error}"
            ))
        })?;
        let params = RequestEnrollmentParams {
            manifest,
            signature: identity.sign_base64(&payload),
        };
        self.runtime
            .call(RuntimeMethod::IntegrationsRequestEnrollment, &params)
            .await
    }

    /// Read the exact pending decision associated with this proved connection.
    ///
    /// # Errors
    ///
    /// Protocol, transport, or Runtime refusal.
    pub async fn watch(
        &mut self,
        pending_id: PendingEnrollmentId,
    ) -> Result<EnrollmentDecision, ClientError> {
        self.runtime
            .call(
                RuntimeMethod::IntegrationsWatchEnrollment,
                &WatchEnrollmentParams { pending_id },
            )
            .await
    }

    /// Read the authenticated integration's current grant.
    ///
    /// # Errors
    ///
    /// Protocol, transport, or Runtime refusal.
    pub async fn grant(&mut self) -> Result<IntegrationGrant, ClientError> {
        self.runtime
            .call(RuntimeMethod::IntegrationsGetGrant, &EmptyParams {})
            .await
    }

    /// Replace the authenticated integration key after local confirmation.
    ///
    /// Keep the replacement identity and request identity until a definite result is received. A retry after an
    /// ambiguous response uses the same request identity and the generation observed before rotation.
    ///
    /// # Errors
    ///
    /// Missing authenticated grant, protocol encoding, transport, or Runtime refusal.
    pub async fn rotate_key(
        &mut self,
        request_id: MutationRequestId,
        expected_key_generation: u64,
        replacement: &IntegrationIdentity,
    ) -> Result<IntegrationCredentials, ClientError> {
        let grant = self.runtime.initialized.grant.as_ref().ok_or_else(|| {
            ClientError::Protocol(
                "integration key rotation requires an authenticated grant".to_owned(),
            )
        })?;
        let mut params = RotateIntegrationKeyParams {
            request_id,
            expected_key_generation,
            new_public_key: replacement.public_key_base64(),
            new_key_proof: String::new(),
        };
        let payload =
            key_rotation_signing_payload(&grant.integration_id, grant.grant_generation, &params)
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "key rotation proof payload cannot be encoded: {error}"
                    ))
                })?;
        params.new_key_proof = replacement.sign_base64(&payload);
        let rotated = self
            .runtime
            .call_mutation(
                RuntimeMethod::IntegrationsRotateKey,
                &params.request_id,
                &params,
            )
            .await?;
        Ok(IntegrationCredentials::new(replacement.clone(), rotated))
    }
}

/// Typed provider inventory methods.
pub struct ProviderClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl ProviderClient<'_> {
    /// Read the immediate structural inventory without starting providers.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including default denial before authorization.
    pub async fn list(&mut self) -> Result<ProviderList, ClientError> {
        self.runtime
            .call(RuntimeMethod::ProvidersList, &EmptyParams {})
            .await
    }

    /// Where each account stands against its limits, by each provider's own latest report.
    ///
    /// An empty list means nothing has reported since the Runtime started, which is different from a limit not
    /// existing.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including missing provider read scope.
    pub async fn usage(&mut self) -> Result<ProviderUsageList, ClientError> {
        self.runtime
            .call(RuntimeMethod::ProvidersUsage, &EmptyParams {})
            .await
    }

    /// Convert this connection into one provider inventory subscription after its initial snapshot.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including missing provider read scope.
    pub async fn watch(&mut self) -> Result<ProviderSubscription<'_>, ClientError> {
        let started: WatchProvidersResult = self
            .runtime
            .call(
                RuntimeMethod::ProvidersWatch,
                &WatchProvidersParams::default(),
            )
            .await?;
        Ok(ProviderSubscription {
            runtime: self.runtime,
            subscription_id: started.subscription_id.clone(),
            started,
        })
    }

    /// Discover one provider's structural lifecycle and event capabilities.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including unavailable providers and bounded discovery timeout.
    pub async fn get_capabilities(
        &mut self,
        provider_id: ProviderId,
    ) -> Result<RuntimeProviderCapabilities, ClientError> {
        self.runtime
            .call(
                RuntimeMethod::ProvidersGetCapabilities,
                &GetProviderCapabilitiesParams { provider_id },
            )
            .await
    }

    /// Explicitly discover one provider's current opaque model catalogue.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including unavailable providers and bounded discovery timeout.
    pub async fn list_models(
        &mut self,
        provider_id: ProviderId,
    ) -> Result<RuntimeModelCatalog, ClientError> {
        self.runtime
            .call(
                RuntimeMethod::ProvidersListModels,
                &ListModelsParams { provider_id },
            )
            .await
    }

    /// Discover one official provider-native page under one exact approved root.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including stale cursors, changed roots, and provider discovery failure.
    pub async fn list_native_sessions(
        &mut self,
        params: ListNativeSessionsParams,
    ) -> Result<NativeSessionCatalogue, ClientError> {
        self.runtime
            .call(RuntimeMethod::ProvidersListNativeSessions, &params)
            .await
    }
}

/// One provider inventory notification.
#[derive(Debug)]
pub enum ProviderNotification {
    /// The complete provider snapshot changed.
    Changed(ProvidersChangedNotification),
    /// The subscription ended with a typed authority or Runtime reason.
    Ended(ProviderWatchEndedNotification),
}

/// One dedicated provider inventory stream borrowed from an initialized Runtime connection.
pub struct ProviderSubscription<'client> {
    runtime: &'client mut RuntimeClient,
    subscription_id: String,
    started: WatchProvidersResult,
}

impl ProviderSubscription<'_> {
    /// Initial provider snapshot acknowledged before notifications begin.
    #[must_use]
    pub const fn started(&self) -> &WatchProvidersResult {
        &self.started
    }

    /// Wait for a changed provider snapshot or terminal reason.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification that violates the selected public revision.
    pub async fn next(&mut self) -> Result<ProviderNotification, ClientError> {
        receive_provider_notification(self.runtime, &self.subscription_id).await
    }
}

async fn receive_provider_notification(
    runtime: &mut RuntimeClient,
    subscription_id: &str,
) -> Result<ProviderNotification, ClientError> {
    let payload = runtime.connection.receive().await?;
    let notification: JsonRpcNotification = serde_json::from_slice(&payload).map_err(|error| {
        ClientError::Protocol(format!(
            "provider notification is not valid public JSON-RPC: {error}"
        ))
    })?;
    if notification.jsonrpc != "2.0" {
        return Err(ClientError::Protocol(
            "provider notification JSON-RPC version is not 2.0".to_owned(),
        ));
    }
    let method = notification
        .method
        .parse::<RuntimeMethod>()
        .map_err(|_| ClientError::Protocol("provider notification method is unknown".to_owned()))?;
    match method {
        RuntimeMethod::ProvidersChanged => {
            let changed: ProvidersChangedNotification = serde_json::from_value(notification.params)
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "provider change notification has the wrong shape: {error}"
                    ))
                })?;
            validate_subscription(
                subscription_id,
                &changed.subscription_id,
                "provider notification target does not match its subscription",
            )?;
            Ok(ProviderNotification::Changed(changed))
        }
        RuntimeMethod::ProvidersWatchEnded => {
            let ended: ProviderWatchEndedNotification = serde_json::from_value(notification.params)
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "provider watch end notification has the wrong shape: {error}"
                    ))
                })?;
            validate_subscription(
                subscription_id,
                &ended.subscription_id,
                "provider notification target does not match its subscription",
            )?;
            Ok(ProviderNotification::Ended(ended))
        }
        _ => Err(ClientError::Protocol(
            "the dedicated provider stream received a different method".to_owned(),
        )),
    }
}

/// One provider notification from a connection-replacing read stream.
#[derive(Debug)]
pub enum ReconnectingProviderNotification {
    /// The complete provider snapshot changed.
    Changed(ProvidersChangedNotification),
    /// Runtime ended the subscription for a typed authority or lifecycle reason.
    Ended(ProviderWatchEndedNotification),
    /// A replacement connection installed a new complete snapshot.
    Reconnected(WatchProvidersResult),
}

/// A provider snapshot stream that owns and replaces its read-only Runtime connection.
pub struct ReconnectingProviderSubscription {
    locator: RuntimeLocator,
    options: ClientOptions,
    policy: ReconnectPolicy,
    runtime: Option<RuntimeClient>,
    subscription_id: String,
    started: WatchProvidersResult,
    terminal: bool,
}

impl ReconnectingProviderSubscription {
    async fn open(
        locator: RuntimeLocator,
        options: ClientOptions,
        policy: ReconnectPolicy,
    ) -> Result<Self, ClientError> {
        let (runtime, started) = open_provider_stream(&locator, &options, policy).await?;
        Ok(Self {
            locator,
            options,
            policy,
            runtime: Some(runtime),
            subscription_id: started.subscription_id.clone(),
            started,
            terminal: false,
        })
    }

    /// Initial complete provider snapshot.
    #[must_use]
    pub const fn started(&self) -> &WatchProvidersResult {
        &self.started
    }

    /// Wait for a changed snapshot, terminal reason, or replacement snapshot.
    ///
    /// # Errors
    ///
    /// A non-retryable protocol failure or reconnect deadline exhaustion.
    pub async fn next(&mut self) -> Result<ReconnectingProviderNotification, ClientError> {
        if self.terminal {
            return Err(ClientError::Protocol(
                "provider subscription already ended".to_owned(),
            ));
        }
        let Some(runtime) = self.runtime.as_mut() else {
            let started = self.reconnect().await?;
            return Ok(ReconnectingProviderNotification::Reconnected(started));
        };
        match receive_provider_notification(runtime, &self.subscription_id).await {
            Ok(ProviderNotification::Changed(changed)) => {
                Ok(ReconnectingProviderNotification::Changed(changed))
            }
            Ok(ProviderNotification::Ended(ended)) => {
                self.runtime = None;
                self.terminal = true;
                Ok(ReconnectingProviderNotification::Ended(ended))
            }
            Err(error) if retryable_connection_failure(&error) => {
                self.runtime = None;
                let started = self.reconnect().await?;
                Ok(ReconnectingProviderNotification::Reconnected(started))
            }
            Err(error) => Err(error),
        }
    }

    async fn reconnect(&mut self) -> Result<WatchProvidersResult, ClientError> {
        let (runtime, started) =
            open_provider_stream(&self.locator, &self.options, self.policy).await?;
        self.subscription_id.clone_from(&started.subscription_id);
        self.started = started.clone();
        self.runtime = Some(runtime);
        Ok(started)
    }
}

/// Typed Runtime-managed session methods.
pub struct SessionClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl SessionClient<'_> {
    /// Read the immediate Runtime-managed session snapshot.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including default denial before authorization.
    pub async fn list(&mut self) -> Result<ManagedSessionList, ClientError> {
        self.runtime
            .call(RuntimeMethod::SessionsList, &EmptyParams {})
            .await
    }

    /// Convert this connection into one managed-session index subscription after its initial snapshot.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including changed root authority or missing list scope.
    pub async fn watch_index(&mut self) -> Result<SessionIndexSubscription<'_>, ClientError> {
        let started: WatchSessionIndexResult = self
            .runtime
            .call(
                RuntimeMethod::SessionsWatchIndex,
                &WatchSessionIndexParams::default(),
            )
            .await?;
        Ok(SessionIndexSubscription {
            runtime: self.runtime,
            subscription_id: started.subscription_id.clone(),
            started,
        })
    }

    /// Read one exact Runtime-managed session descriptor.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including hidden or missing sessions and changed root authority.
    pub async fn get(
        &mut self,
        session_id: RuntimeSessionId,
    ) -> Result<SessionDescriptor, ClientError> {
        self.runtime
            .call(RuntimeMethod::SessionsGet, &GetSessionParams { session_id })
            .await
    }

    /// Start one fresh provider-native session under an exact authorized workspace.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including stale provider inventory, root denial, and ambiguous provider I/O.
    pub async fn start(
        &mut self,
        params: &StartSessionParams,
    ) -> Result<SessionOpenResult, ClientError> {
        self.runtime
            .call_mutation(RuntimeMethod::SessionsStart, &params.request_id, params)
            .await
    }

    /// Adopt one exact provider-native catalogue observation into Runtime supervision.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including expired adoption proof and ambiguous provider I/O.
    pub async fn adopt_native(
        &mut self,
        params: &AdoptNativeSessionParams,
    ) -> Result<SessionOpenResult, ClientError> {
        self.runtime
            .call_mutation(
                RuntimeMethod::SessionsAdoptNative,
                &params.request_id,
                params,
            )
            .await
    }

    /// Heat one observed cold Runtime-managed session.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including changed lifecycle generation and ambiguous provider I/O.
    pub async fn resume(
        &mut self,
        params: &ResumeSessionParams,
    ) -> Result<SessionOpenResult, ClientError> {
        self.runtime
            .call_mutation(RuntimeMethod::SessionsResume, &params.request_id, params)
            .await
    }

    /// Atomically acquire one renewable write-control lease against the observed session generation.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including stale lifecycle and control conflicts.
    pub async fn acquire_control(
        &mut self,
        params: &AcquireControlParams,
    ) -> Result<ControlLease, ClientError> {
        self.runtime
            .call_mutation(
                RuntimeMethod::SessionsAcquireControl,
                &params.request_id,
                params,
            )
            .await
    }

    /// Renew one exact current lease generation.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including expired or stale leases.
    pub async fn renew_control(
        &mut self,
        params: &ControlLeaseParams,
    ) -> Result<ControlLease, ClientError> {
        self.runtime
            .call_mutation(
                RuntimeMethod::SessionsRenewControl,
                &params.request_id,
                params,
            )
            .await
    }

    /// Release one exact current lease generation.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including expired or stale leases.
    pub async fn release_control(
        &mut self,
        params: &ControlLeaseParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(
                RuntimeMethod::SessionsReleaseControl,
                &params.request_id,
                params,
            )
            .await?;
        Ok(())
    }

    /// Forward caller-owned input unchanged under one exact lease generation.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures. An ambiguous provider boundary returns `outcomeUnknown`.
    pub async fn submit_input(&mut self, params: &SubmitInputParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(
                RuntimeMethod::SessionsSubmitInput,
                &params.request_id,
                params,
            )
            .await?;
        Ok(())
    }

    /// Forward caller-owned typed blocks (text and images) unchanged under one exact lease generation.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including the provider's own loud refusal when it cannot
    /// take an attachment. An ambiguous provider boundary returns `outcomeUnknown`.
    pub async fn submit_blocks(&mut self, params: &SubmitBlocksParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(
                RuntimeMethod::SessionsSubmitBlocks,
                &params.request_id,
                params,
            )
            .await?;
        Ok(())
    }

    /// Relay the operator's model choice through the provider's own switch surface.
    ///
    /// What the session actually answers with stays the provider's word, arriving on the event stream.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including the provider's own loud refusal when its surface cannot
    /// carry the request.
    pub async fn set_model(&mut self, params: &SetModelParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::SessionsSetModel, &params.request_id, params)
            .await?;
        Ok(())
    }

    /// Ask the provider to run under a different permission mode, under the caller's control lease.
    ///
    /// Whether it changed stays the provider's word, arriving on the event stream as its own event.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including the refusal when the named mode is not one the provider
    /// accepts a runtrol switch to.
    pub async fn set_mode(&mut self, params: &SetModeParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::SessionsSetMode, &params.request_id, params)
            .await?;
        Ok(())
    }

    /// Ask the provider to interrupt one exact controlled session.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures. The request never invents a turn outcome.
    pub async fn interrupt(&mut self, params: &ControlLeaseParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::SessionsInterrupt, &params.request_id, params)
            .await?;
        Ok(())
    }

    /// Release one exact idle provider process while retaining its managed session pointer.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including stale lifecycle, lease conflicts, and ambiguous cleanup.
    pub async fn cool(&mut self, params: &CoolSessionParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::SessionsCool, &params.request_id, params)
            .await?;
        Ok(())
    }

    /// Forget one Runtime pointer after the operator approves that exact removal locally.
    ///
    /// The first call normally returns `presenceRequired`. Retrying the same request identity after local approval
    /// completes the idempotent public mutation without touching provider-owned conversation state.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including missing local confirmation or changed session generation.
    pub async fn forget(&mut self, params: &ForgetSessionParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::SessionsForget, &params.request_id, params)
            .await?;
        Ok(())
    }

    /// Delete one provider-native conversation through the provider's own surface.
    ///
    /// Runtime relays the request and stores nothing; a provider without such a surface refuses as
    /// `capabilityUnavailable`, and a conversation Runtime supervises is refused until it is forgotten.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including the provider's own refusal.
    pub async fn delete_native(
        &mut self,
        params: &DeleteNativeSessionParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(
                RuntimeMethod::SessionsDeleteNative,
                &params.request_id,
                params,
            )
            .await?;
        Ok(())
    }

    /// Archive one provider-native conversation through the provider's own surface.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including the provider's own refusal.
    pub async fn archive_native(
        &mut self,
        params: &ArchiveNativeSessionParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(
                RuntimeMethod::SessionsArchiveNative,
                &params.request_id,
                params,
            )
            .await?;
        Ok(())
    }

    /// Convert this connection into one bounded event subscription after the acknowledgement.
    ///
    /// The returned borrow prevents another request from sharing the dedicated stream connection.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including an invalid or unavailable replay cursor.
    pub async fn watch_events<'client>(
        &'client mut self,
        params: &WatchEventsParams,
    ) -> Result<EventSubscription<'client>, ClientError> {
        let started: WatchEventsResult = self
            .runtime
            .call(RuntimeMethod::SessionsWatchEvents, params)
            .await?;
        Ok(EventSubscription {
            runtime: self.runtime,
            subscription_id: started.subscription_id.clone(),
            session_id: started.session_id.clone(),
            started,
        })
    }
}

/// Typed structured provider approval methods.
pub struct ApprovalClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl ApprovalClient<'_> {
    /// Read every pending normalized request under one exact current control lease.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including missing output scope and stale control authority.
    pub async fn list_pending(
        &mut self,
        params: &ListPendingApprovalsParams,
    ) -> Result<PendingApprovalList, ClientError> {
        self.runtime
            .call(RuntimeMethod::ApprovalsListPending, params)
            .await
    }

    /// Answer one exact pending request without accepting caller-supplied risk.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including stale subjects, unavailable options, expiry, and scope denial.
    pub async fn respond(&mut self, params: &RespondApprovalParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .runtime
            .call_mutation(RuntimeMethod::ApprovalsRespond, &params.request_id, params)
            .await?;
        Ok(())
    }
}

/// One managed-session index notification.
#[derive(Debug)]
pub enum SessionIndexNotification {
    /// The authorized snapshot changed.
    Changed(SessionIndexChangedNotification),
    /// The subscription ended with a typed authority or Runtime reason.
    Ended(SessionIndexEndedNotification),
}

/// One dedicated managed-session index stream borrowed from an initialized Runtime connection.
pub struct SessionIndexSubscription<'client> {
    runtime: &'client mut RuntimeClient,
    subscription_id: String,
    started: WatchSessionIndexResult,
}

impl SessionIndexSubscription<'_> {
    /// Initial authorized snapshot acknowledged before notifications begin.
    #[must_use]
    pub const fn started(&self) -> &WatchSessionIndexResult {
        &self.started
    }

    /// Wait for a changed snapshot or terminal reason.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification that violates the selected public revision.
    pub async fn next(&mut self) -> Result<SessionIndexNotification, ClientError> {
        receive_session_index_notification(self.runtime, &self.subscription_id).await
    }
}

async fn receive_session_index_notification(
    runtime: &mut RuntimeClient,
    subscription_id: &str,
) -> Result<SessionIndexNotification, ClientError> {
    let payload = runtime.connection.receive().await?;
    let notification: JsonRpcNotification = serde_json::from_slice(&payload).map_err(|error| {
        ClientError::Protocol(format!(
            "session index notification is not valid public JSON-RPC: {error}"
        ))
    })?;
    if notification.jsonrpc != "2.0" {
        return Err(ClientError::Protocol(
            "session index notification JSON-RPC version is not 2.0".to_owned(),
        ));
    }
    let method = notification.method.parse::<RuntimeMethod>().map_err(|_| {
        ClientError::Protocol("session index notification method is unknown".to_owned())
    })?;
    match method {
        RuntimeMethod::SessionsIndexChanged => {
            let changed: SessionIndexChangedNotification =
                serde_json::from_value(notification.params).map_err(|error| {
                    ClientError::Protocol(format!(
                        "session index change notification has the wrong shape: {error}"
                    ))
                })?;
            validate_subscription(
                subscription_id,
                &changed.subscription_id,
                "session index notification target does not match its subscription",
            )?;
            Ok(SessionIndexNotification::Changed(changed))
        }
        RuntimeMethod::SessionsIndexEnded => {
            let ended: SessionIndexEndedNotification = serde_json::from_value(notification.params)
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "session index end notification has the wrong shape: {error}"
                    ))
                })?;
            validate_subscription(
                subscription_id,
                &ended.subscription_id,
                "session index notification target does not match its subscription",
            )?;
            Ok(SessionIndexNotification::Ended(ended))
        }
        _ => Err(ClientError::Protocol(
            "the dedicated session index stream received a different method".to_owned(),
        )),
    }
}

/// One session-index notification from a connection-replacing read stream.
#[derive(Debug)]
pub enum ReconnectingSessionIndexNotification {
    /// The complete authorized session snapshot changed.
    Changed(SessionIndexChangedNotification),
    /// Runtime ended the subscription for a typed authority or lifecycle reason.
    Ended(SessionIndexEndedNotification),
    /// A replacement connection installed a new complete authorized snapshot.
    Reconnected(WatchSessionIndexResult),
}

/// A managed-session snapshot stream that owns and replaces its read-only Runtime connection.
pub struct ReconnectingSessionIndexSubscription {
    locator: RuntimeLocator,
    options: ClientOptions,
    policy: ReconnectPolicy,
    runtime: Option<RuntimeClient>,
    subscription_id: String,
    started: WatchSessionIndexResult,
    terminal: bool,
}

impl ReconnectingSessionIndexSubscription {
    async fn open(
        locator: RuntimeLocator,
        options: ClientOptions,
        policy: ReconnectPolicy,
    ) -> Result<Self, ClientError> {
        let (runtime, started) = open_session_index_stream(&locator, &options, policy).await?;
        Ok(Self {
            locator,
            options,
            policy,
            runtime: Some(runtime),
            subscription_id: started.subscription_id.clone(),
            started,
            terminal: false,
        })
    }

    /// Initial complete authorized session snapshot.
    #[must_use]
    pub const fn started(&self) -> &WatchSessionIndexResult {
        &self.started
    }

    /// Wait for a changed snapshot, terminal reason, or replacement snapshot.
    ///
    /// # Errors
    ///
    /// A non-retryable protocol failure or reconnect deadline exhaustion.
    pub async fn next(&mut self) -> Result<ReconnectingSessionIndexNotification, ClientError> {
        if self.terminal {
            return Err(ClientError::Protocol(
                "session index subscription already ended".to_owned(),
            ));
        }
        let Some(runtime) = self.runtime.as_mut() else {
            let started = self.reconnect().await?;
            return Ok(ReconnectingSessionIndexNotification::Reconnected(started));
        };
        match receive_session_index_notification(runtime, &self.subscription_id).await {
            Ok(SessionIndexNotification::Changed(changed)) => {
                Ok(ReconnectingSessionIndexNotification::Changed(changed))
            }
            Ok(SessionIndexNotification::Ended(ended)) => {
                self.runtime = None;
                self.terminal = true;
                Ok(ReconnectingSessionIndexNotification::Ended(ended))
            }
            Err(error) if retryable_connection_failure(&error) => {
                self.runtime = None;
                let started = self.reconnect().await?;
                Ok(ReconnectingSessionIndexNotification::Reconnected(started))
            }
            Err(error) => Err(error),
        }
    }

    async fn reconnect(&mut self) -> Result<WatchSessionIndexResult, ClientError> {
        let (runtime, started) =
            open_session_index_stream(&self.locator, &self.options, self.policy).await?;
        self.subscription_id.clone_from(&started.subscription_id);
        self.started = started.clone();
        self.runtime = Some(runtime);
        Ok(started)
    }
}

/// One dedicated bounded event stream borrowed from an initialized Runtime connection.
pub struct EventSubscription<'client> {
    runtime: &'client mut RuntimeClient,
    subscription_id: String,
    session_id: RuntimeSessionId,
    started: WatchEventsResult,
}

impl EventSubscription<'_> {
    /// Exact replay and live boundary acknowledged before notifications begin.
    #[must_use]
    pub const fn started(&self) -> &WatchEventsResult {
        &self.started
    }

    /// Wait for the next normalized event or explicit lag boundary.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification that violates the selected public revision.
    pub async fn next(&mut self) -> Result<SessionNotification, ClientError> {
        receive_session_notification(self.runtime, &self.subscription_id, &self.session_id).await
    }
}

async fn receive_session_notification(
    runtime: &mut RuntimeClient,
    subscription_id: &str,
    session_id: &RuntimeSessionId,
) -> Result<SessionNotification, ClientError> {
    let payload = runtime.connection.receive().await?;
    let notification: JsonRpcNotification = serde_json::from_slice(&payload).map_err(|error| {
        ClientError::Protocol(format!(
            "session notification is not valid public JSON-RPC: {error}"
        ))
    })?;
    if notification.jsonrpc != "2.0" {
        return Err(ClientError::Protocol(
            "session notification JSON-RPC version is not 2.0".to_owned(),
        ));
    }
    let method = notification
        .method
        .parse::<RuntimeMethod>()
        .map_err(|_| ClientError::Protocol("session notification method is unknown".to_owned()))?;
    match method {
        RuntimeMethod::SessionsEvent => {
            let event: RuntimeEventNotification = serde_json::from_value(notification.params)
                .map_err(|error| {
                    ClientError::Protocol(format!(
                        "session event notification has the wrong shape: {error}"
                    ))
                })?;
            validate_session_notification_target(
                subscription_id,
                session_id,
                &event.subscription_id,
                &event.session_id,
            )?;
            Ok(SessionNotification::Event(event))
        }
        RuntimeMethod::SessionsLagged => {
            let lagged: LaggedNotification =
                serde_json::from_value(notification.params).map_err(|error| {
                    ClientError::Protocol(format!(
                        "session lag notification has the wrong shape: {error}"
                    ))
                })?;
            validate_session_notification_target(
                subscription_id,
                session_id,
                &lagged.subscription_id,
                &lagged.session_id,
            )?;
            Ok(SessionNotification::Lagged(lagged))
        }
        RuntimeMethod::Initialize
        | RuntimeMethod::Initialized
        | RuntimeMethod::Challenge
        | RuntimeMethod::IntegrationsRequestEnrollment
        | RuntimeMethod::IntegrationsWatchEnrollment
        | RuntimeMethod::IntegrationsGetGrant
        | RuntimeMethod::IntegrationsRotateKey
        | RuntimeMethod::ProvidersList
        | RuntimeMethod::ProvidersUsage
        | RuntimeMethod::ProvidersWatch
        | RuntimeMethod::ProvidersGetCapabilities
        | RuntimeMethod::ProvidersListModels
        | RuntimeMethod::ProvidersListNativeSessions
        | RuntimeMethod::SessionsList
        | RuntimeMethod::SessionsWatchIndex
        | RuntimeMethod::SessionsGet
        | RuntimeMethod::SessionsStart
        | RuntimeMethod::SessionsAdoptNative
        | RuntimeMethod::SessionsResume
        | RuntimeMethod::SessionsAcquireControl
        | RuntimeMethod::SessionsRenewControl
        | RuntimeMethod::SessionsReleaseControl
        | RuntimeMethod::SessionsSubmitInput
        | RuntimeMethod::SessionsSubmitBlocks
        | RuntimeMethod::SessionsSetModel
        | RuntimeMethod::SessionsSetMode
        | RuntimeMethod::SessionsWatchEvents
        | RuntimeMethod::SessionsInterrupt
        | RuntimeMethod::SessionsCool
        | RuntimeMethod::SessionsForget
        | RuntimeMethod::SessionsDeleteNative
        | RuntimeMethod::SessionsArchiveNative
        | RuntimeMethod::ApprovalsListPending
        | RuntimeMethod::ApprovalsRespond
        | RuntimeMethod::SessionsIndexChanged
        | RuntimeMethod::SessionsIndexEnded
        | RuntimeMethod::ProvidersChanged
        | RuntimeMethod::ProvidersWatchEnded
        | RuntimeMethod::PanicStop => Err(ClientError::Protocol(
            "the dedicated session stream received a non-event method".to_owned(),
        )),
    }
}

fn validate_session_notification_target(
    expected_subscription_id: &str,
    expected_session_id: &RuntimeSessionId,
    subscription_id: &str,
    session_id: &RuntimeSessionId,
) -> Result<(), ClientError> {
    if subscription_id != expected_subscription_id || session_id != expected_session_id {
        return Err(ClientError::Protocol(
            "session notification target does not match its subscription".to_owned(),
        ));
    }
    Ok(())
}

fn validate_subscription(
    expected: &str,
    actual: &str,
    mismatch: &'static str,
) -> Result<(), ClientError> {
    if actual != expected {
        return Err(ClientError::Protocol(mismatch.to_owned()));
    }
    Ok(())
}

/// One item on a dedicated public session stream.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionNotification {
    /// One normalized provider-neutral event and its next reconnect boundary.
    Event(RuntimeEventNotification),
    /// The subscriber fell behind the bounded queue and must reconnect from the named boundary.
    Lagged(LaggedNotification),
}

/// One notification from an event stream that can replace its read-only connection.
#[derive(Clone, Debug, PartialEq)]
pub enum ReconnectingSessionNotification {
    /// One normalized event. The consumer must accept its cursor before reading again.
    Event(RuntimeEventNotification),
    /// The bounded live queue was lost. Reconnect begins from the supplied safe boundary.
    Lagged(LaggedNotification),
    /// A replacement connection installed a new replay and live boundary.
    Reconnected(WatchEventsResult),
}

/// A read-only event stream that restores only an explicitly accepted cursor.
pub struct ReconnectingEventSubscription {
    locator: RuntimeLocator,
    options: ClientOptions,
    policy: ReconnectPolicy,
    session_id: RuntimeSessionId,
    accepted: Option<EventCursor>,
    pending: Option<EventCursor>,
    runtime: Option<RuntimeClient>,
    subscription_id: String,
    started: WatchEventsResult,
}

impl ReconnectingEventSubscription {
    async fn open(
        locator: RuntimeLocator,
        options: ClientOptions,
        params: WatchEventsParams,
        policy: ReconnectPolicy,
    ) -> Result<Self, ClientError> {
        let accepted = params.after;
        let (runtime, started) = open_event_stream(
            &locator,
            &options,
            &params.session_id,
            accepted.clone(),
            policy,
        )
        .await?;
        Ok(Self {
            locator,
            options,
            policy,
            session_id: params.session_id,
            accepted,
            pending: None,
            runtime: Some(runtime),
            subscription_id: started.subscription_id.clone(),
            started,
        })
    }

    /// Initial replay and live boundary.
    #[must_use]
    pub const fn started(&self) -> &WatchEventsResult {
        &self.started
    }

    /// Mark one delivered event as consumed and safe for reconnect.
    ///
    /// # Errors
    ///
    /// The cursor is not the exact pending event boundary.
    pub fn accept(&mut self, next_expected: &EventCursor) -> Result<(), ClientError> {
        if self.pending.as_ref() != Some(next_expected) {
            return Err(ClientError::Protocol(
                "accepted event cursor does not match the pending event".to_owned(),
            ));
        }
        self.accepted = Some(next_expected.clone());
        self.pending = None;
        Ok(())
    }

    /// Wait for an event, lag boundary, or successful replacement connection.
    ///
    /// # Errors
    ///
    /// An unaccepted event, a non-retryable protocol failure, or reconnect deadline exhaustion.
    pub async fn next(&mut self) -> Result<ReconnectingSessionNotification, ClientError> {
        if self.pending.is_some() {
            return Err(ClientError::Protocol(
                "accept the current event before reading another one".to_owned(),
            ));
        }
        let Some(runtime) = self.runtime.as_mut() else {
            let started = self.reconnect().await?;
            return Ok(ReconnectingSessionNotification::Reconnected(started));
        };
        match receive_session_notification(runtime, &self.subscription_id, &self.session_id).await {
            Ok(SessionNotification::Event(event)) => {
                self.pending = Some(event.next_expected.clone());
                Ok(ReconnectingSessionNotification::Event(event))
            }
            Ok(SessionNotification::Lagged(lagged)) => {
                self.accepted = Some(lagged.next_expected.clone());
                self.runtime = None;
                Ok(ReconnectingSessionNotification::Lagged(lagged))
            }
            Err(error) if retryable_connection_failure(&error) => {
                self.runtime = None;
                let started = self.reconnect().await?;
                Ok(ReconnectingSessionNotification::Reconnected(started))
            }
            Err(error) => Err(error),
        }
    }

    async fn reconnect(&mut self) -> Result<WatchEventsResult, ClientError> {
        let (runtime, started) = open_event_stream(
            &self.locator,
            &self.options,
            &self.session_id,
            self.accepted.clone(),
            self.policy,
        )
        .await?;
        self.subscription_id.clone_from(&started.subscription_id);
        self.started = started.clone();
        self.runtime = Some(runtime);
        Ok(started)
    }
}

async fn open_event_stream(
    locator: &RuntimeLocator,
    options: &ClientOptions,
    session_id: &RuntimeSessionId,
    after: Option<EventCursor>,
    policy: ReconnectPolicy,
) -> Result<(RuntimeClient, WatchEventsResult), ClientError> {
    open_read_stream(locator, options, policy, || {
        let params = WatchEventsParams {
            session_id: session_id.clone(),
            after: after.clone(),
        };
        async move |runtime: &mut RuntimeClient| {
            runtime
                .call(RuntimeMethod::SessionsWatchEvents, &params)
                .await
        }
    })
    .await
}

async fn open_provider_stream(
    locator: &RuntimeLocator,
    options: &ClientOptions,
    policy: ReconnectPolicy,
) -> Result<(RuntimeClient, WatchProvidersResult), ClientError> {
    open_read_stream(locator, options, policy, || {
        async move |runtime: &mut RuntimeClient| {
            runtime
                .call(
                    RuntimeMethod::ProvidersWatch,
                    &WatchProvidersParams::default(),
                )
                .await
        }
    })
    .await
}

async fn open_session_index_stream(
    locator: &RuntimeLocator,
    options: &ClientOptions,
    policy: ReconnectPolicy,
) -> Result<(RuntimeClient, WatchSessionIndexResult), ClientError> {
    open_read_stream(locator, options, policy, || {
        async move |runtime: &mut RuntimeClient| {
            runtime
                .call(
                    RuntimeMethod::SessionsWatchIndex,
                    &WatchSessionIndexParams::default(),
                )
                .await
        }
    })
    .await
}

async fn open_read_stream<R, Factory, Operation>(
    locator: &RuntimeLocator,
    options: &ClientOptions,
    policy: ReconnectPolicy,
    mut factory: Factory,
) -> Result<(RuntimeClient, R), ClientError>
where
    Factory: FnMut() -> Operation,
    Operation: for<'client> AsyncFnOnce(&'client mut RuntimeClient) -> Result<R, ClientError>,
{
    let deadline = tokio::time::Instant::now() + policy.deadline;
    let mut delay = policy.initial_delay;
    loop {
        let attempt = async {
            let mut runtime = locator.connect(options.clone()).await?;
            let started = factory()(&mut runtime).await?;
            Ok((runtime, started))
        }
        .await;
        match attempt {
            Ok(opened) => return Ok(opened),
            Err(error) if retryable_connection_failure(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(error);
                }
                tokio::time::sleep(jittered(delay).min(deadline - now)).await;
                delay = delay.saturating_mul(2).min(policy.maximum_delay);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

    #[cfg(windows)]
    mod fake_transport {
        use std::sync::atomic::{AtomicU64, Ordering};
        use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

        static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(1);

        pub(super) type Stream = NamedPipeServer;

        pub(super) struct Listener {
            endpoint: String,
            waiting: Option<NamedPipeServer>,
        }

        impl Listener {
            pub(super) fn bind() -> (Self, String) {
                let endpoint = format!(
                    r"\\.\pipe\runtrol-runtime-client-reconnect-{}-{}",
                    std::process::id(),
                    NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
                );
                let waiting = ServerOptions::new()
                    .first_pipe_instance(true)
                    .reject_remote_clients(true)
                    .create(&endpoint)
                    .expect("create reconnect test pipe");
                (
                    Self {
                        endpoint: endpoint.clone(),
                        waiting: Some(waiting),
                    },
                    endpoint,
                )
            }

            pub(super) async fn accept(&mut self) -> Stream {
                let waiting = match self.waiting.take() {
                    Some(waiting) => waiting,
                    None => ServerOptions::new()
                        .reject_remote_clients(true)
                        .create(&self.endpoint)
                        .expect("create replacement reconnect test pipe"),
                };
                waiting.connect().await.expect("accept reconnect client");
                waiting
            }
        }
    }

    #[cfg(unix)]
    mod fake_transport {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};
        use tokio::net::{UnixListener, UnixStream};

        static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(1);

        pub(super) type Stream = UnixStream;

        pub(super) struct Listener {
            inner: UnixListener,
            directory: PathBuf,
        }

        impl Listener {
            pub(super) fn bind() -> (Self, String) {
                let directory = std::env::temp_dir().join(format!(
                    "runtrol-runtime-client-reconnect-{}-{}",
                    std::process::id(),
                    NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
                ));
                drop(std::fs::remove_dir_all(&directory));
                std::fs::create_dir_all(&directory).expect("create reconnect test directory");
                let endpoint = directory.join("runtrol-runtime.sock");
                let inner = UnixListener::bind(&endpoint).expect("bind reconnect test socket");
                (
                    Self { inner, directory },
                    endpoint.to_string_lossy().into_owned(),
                )
            }

            pub(super) async fn accept(&mut self) -> Stream {
                self.inner
                    .accept()
                    .await
                    .expect("accept reconnect client")
                    .0
            }
        }

        impl Drop for Listener {
            fn drop(&mut self) {
                drop(std::fs::remove_dir_all(&self.directory));
            }
        }
    }

    async fn receive_test_frame(stream: &mut (impl AsyncRead + Unpin)) -> Vec<u8> {
        let mut header = [0_u8; 4];
        stream
            .read_exact(&mut header)
            .await
            .expect("read test frame header");
        let length = usize::try_from(u32::from_be_bytes(header)).expect("test frame length");
        let mut payload = vec![0_u8; length];
        stream
            .read_exact(&mut payload)
            .await
            .expect("read test frame payload");
        payload
    }

    async fn send_test_json(stream: &mut (impl AsyncWrite + Unpin), value: &impl Serialize) {
        let payload = serde_json::to_vec(value).expect("encode test frame");
        let length = u32::try_from(payload.len()).expect("bounded test frame length");
        stream
            .write_all(&length.to_be_bytes())
            .await
            .expect("write test frame header");
        stream
            .write_all(&payload)
            .await
            .expect("write test frame payload");
        stream.flush().await.expect("flush test frame");
    }

    async fn serve_mutation_disconnect_fixture(
        mut stream: fake_transport::Stream,
        instance_id: &str,
    ) -> JsonRpcRequest {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock follows Unix epoch");
        let challenge = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: RuntimeMethod::Challenge.to_string(),
            params: serde_json::to_value(ServerChallenge {
                instance_id: instance_id.to_owned(),
                nonce_id: "nonce_0123456789abcdef0123456789abcdef".to_owned(),
                nonce: Base64UrlUnpadded::encode_string(&[3; 32]),
                expires_at_ms: u64::try_from(now.as_millis())
                    .expect("test milliseconds fit u64")
                    .saturating_add(1_000),
            })
            .expect("encode challenge"),
        };
        send_test_json(&mut stream, &challenge).await;

        let initialize: JsonRpcRequest =
            serde_json::from_slice(&receive_test_frame(&mut stream).await)
                .expect("decode initialize request");
        let initialized = InitializeResult {
            selected_revision: runtrol_runtime_protocol::REVISION_2026_08_13,
            runtime: runtrol_runtime_protocol::RuntimeInstance {
                instance_id: instance_id.to_owned(),
                version: "0.1.1".to_owned(),
                platform: "test".to_owned(),
                build_digest: None,
            },
            server_capabilities: runtrol_runtime_protocol::RuntimeCapabilities {
                integration_enrollment: true,
                provider_inventory: true,
                managed_session_list: true,
                model_discovery: true,
                native_session_catalogue: true,
                session_control: true,
                session_events: true,
            },
            limits: runtrol_runtime_protocol::RuntimeLimits::default(),
            grant: None,
        };
        send_test_json(
            &mut stream,
            &JsonRpcResponse::Success(SuccessResponse {
                jsonrpc: "2.0".to_owned(),
                id: initialize.id,
                result: serde_json::to_value(initialized).expect("encode initialization result"),
            }),
        )
        .await;

        let initialized: JsonRpcNotification =
            serde_json::from_slice(&receive_test_frame(&mut stream).await)
                .expect("decode initialized notification");
        assert_eq!(initialized.method, RuntimeMethod::Initialized.to_string());
        serde_json::from_slice(&receive_test_frame(&mut stream).await)
            .expect("decode mutation request")
    }

    async fn serve_reconnect_fixture(
        mut stream: fake_transport::Stream,
        instance_id: &str,
        expected_after: EventCursor,
        next_expected: EventCursor,
        session_id: &RuntimeSessionId,
        subscription_id: &str,
        send_event: bool,
    ) -> WatchEventsParams {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock follows Unix epoch");
        let challenge = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: RuntimeMethod::Challenge.to_string(),
            params: serde_json::to_value(ServerChallenge {
                instance_id: instance_id.to_owned(),
                nonce_id: "nonce_0123456789abcdef0123456789abcdef".to_owned(),
                nonce: Base64UrlUnpadded::encode_string(&[3; 32]),
                expires_at_ms: u64::try_from(now.as_millis())
                    .expect("test milliseconds fit u64")
                    .saturating_add(1_000),
            })
            .expect("encode challenge"),
        };
        send_test_json(&mut stream, &challenge).await;

        let initialize: JsonRpcRequest =
            serde_json::from_slice(&receive_test_frame(&mut stream).await)
                .expect("decode initialize request");
        assert_eq!(initialize.method, RuntimeMethod::Initialize.to_string());
        let initialized = runtrol_runtime_protocol::InitializeResult {
            selected_revision: runtrol_runtime_protocol::REVISION_2026_08_13,
            runtime: runtrol_runtime_protocol::RuntimeInstance {
                instance_id: instance_id.to_owned(),
                version: "0.1.1".to_owned(),
                platform: "test".to_owned(),
                build_digest: None,
            },
            server_capabilities: runtrol_runtime_protocol::RuntimeCapabilities {
                integration_enrollment: true,
                provider_inventory: true,
                managed_session_list: true,
                model_discovery: true,
                native_session_catalogue: true,
                session_control: true,
                session_events: true,
            },
            limits: runtrol_runtime_protocol::RuntimeLimits::default(),
            grant: None,
        };
        send_test_json(
            &mut stream,
            &JsonRpcResponse::Success(SuccessResponse {
                jsonrpc: "2.0".to_owned(),
                id: initialize.id,
                result: serde_json::to_value(initialized).expect("encode initialization result"),
            }),
        )
        .await;

        let initialized: JsonRpcNotification =
            serde_json::from_slice(&receive_test_frame(&mut stream).await)
                .expect("decode initialized notification");
        assert_eq!(initialized.method, RuntimeMethod::Initialized.to_string());
        let watch: JsonRpcRequest = serde_json::from_slice(&receive_test_frame(&mut stream).await)
            .expect("decode watch request");
        assert_eq!(watch.method, RuntimeMethod::SessionsWatchEvents.to_string());
        let params: WatchEventsParams =
            serde_json::from_value(watch.params).expect("decode watch parameters");
        assert_eq!(params.after, Some(expected_after.clone()));
        let started = WatchEventsResult {
            subscription_id: subscription_id.to_owned(),
            session_id: session_id.clone(),
            starts_at: expected_after,
            live_at: next_expected.clone(),
            gap: None,
        };
        send_test_json(
            &mut stream,
            &JsonRpcResponse::Success(SuccessResponse {
                jsonrpc: "2.0".to_owned(),
                id: watch.id,
                result: serde_json::to_value(started).expect("encode watch result"),
            }),
        )
        .await;
        if send_event {
            send_test_json(
                &mut stream,
                &JsonRpcNotification {
                    jsonrpc: "2.0".to_owned(),
                    method: RuntimeMethod::SessionsEvent.to_string(),
                    params: serde_json::to_value(RuntimeEventNotification {
                        subscription_id: subscription_id.to_owned(),
                        session_id: session_id.clone(),
                        event_revision: runtrol_runtime_protocol::REVISION_2026_08_13,
                        event: serde_json::json!({"type": "fixture"}),
                        next_expected,
                    })
                    .expect("encode event notification"),
                },
            )
            .await;
        }
        params
    }

    #[test]
    fn client_options_contain_no_endpoint_or_provider_choice() {
        let options = ClientOptions::new("fixture", "1.0.0");
        assert_eq!(options.name, "fixture");
        assert_eq!(options.version, "1.0.0");
    }

    #[test]
    fn reconnect_policy_is_bounded_and_retries_only_transient_failures() {
        assert!(
            ReconnectPolicy::new(
                Duration::from_millis(10),
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .is_ok()
        );
        assert!(
            ReconnectPolicy::new(
                Duration::ZERO,
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .is_err()
        );
        assert!(retryable_connection_failure(&ClientError::Transport {
            doing: "testing reconnect",
            detail: "temporarily unavailable".to_owned(),
        }));
        assert!(!retryable_connection_failure(&ClientError::Protocol(
            "closed contract failure".to_owned(),
        )));
        let base = Duration::from_millis(100);
        let delayed = jittered(base);
        assert!(delayed >= Duration::from_millis(75));
        assert!(delayed <= Duration::from_millis(125));
    }

    #[tokio::test]
    async fn reconnecting_event_stream_restores_only_the_accepted_cursor() {
        let (mut listener, endpoint) = fake_transport::Listener::bind();
        let instance_id = "rtm_0123456789abcdef0123456789abcdef";
        let session_id = RuntimeSessionId::new("session_reconnect_fixture");
        let initial = EventCursor {
            stream: "019c0000-0000-7000-8000-000000000001".to_owned(),
            epoch: 2,
            seq: 4,
        };
        let accepted = EventCursor {
            stream: initial.stream.clone(),
            epoch: initial.epoch,
            seq: 5,
        };
        let server = tokio::spawn({
            let initial = initial.clone();
            let accepted = accepted.clone();
            let session_id = session_id.clone();
            async move {
                let first = listener.accept().await;
                let first_params = serve_reconnect_fixture(
                    first,
                    instance_id,
                    initial,
                    accepted.clone(),
                    &session_id,
                    "sub_first",
                    true,
                )
                .await;
                let second = listener.accept().await;
                let second_params = serve_reconnect_fixture(
                    second,
                    instance_id,
                    accepted.clone(),
                    accepted,
                    &session_id,
                    "sub_second",
                    false,
                )
                .await;
                (first_params, second_params)
            }
        });

        let locator = RuntimeLocator::for_testing_endpoint(instance_id, endpoint, "0.1.1");
        let policy = ReconnectPolicy::new(
            Duration::from_millis(1),
            Duration::from_millis(5),
            Duration::from_secs(2),
        )
        .expect("valid reconnect policy");
        let mut subscription = locator
            .watch_events_with_reconnect(
                ClientOptions::new("reconnect fixture", "1.0.0"),
                WatchEventsParams {
                    session_id,
                    after: Some(initial.clone()),
                },
                policy,
            )
            .await
            .expect("open reconnecting event stream");
        let event = subscription.next().await.expect("receive first event");
        assert!(matches!(
            event,
            ReconnectingSessionNotification::Event(RuntimeEventNotification {
                ref next_expected,
                ..
            }) if next_expected == &accepted
        ));
        assert!(matches!(
            subscription.next().await,
            Err(ClientError::Protocol(_))
        ));
        subscription.accept(&accepted).expect("accept exact cursor");
        let reconnected = subscription.next().await.expect("reconnect event stream");
        assert!(matches!(
            reconnected,
            ReconnectingSessionNotification::Reconnected(WatchEventsResult {
                ref starts_at,
                ..
            }) if starts_at == &accepted
        ));
        let (first_params, second_params) = server.await.expect("join reconnect fixture");
        assert_eq!(first_params.after, Some(initial));
        assert_eq!(second_params.after, Some(accepted));
    }

    #[tokio::test]
    async fn mutation_transport_loss_returns_original_request_identity() {
        let (mut listener, endpoint) = fake_transport::Listener::bind();
        let instance_id = "rtm_1123456789abcdef0123456789abcdef";
        let request_id: MutationRequestId = "019c2b97-5f29-7b00-8000-000000000004"
            .parse()
            .expect("valid fixture mutation identity");
        let server = tokio::spawn(async move {
            let stream = listener.accept().await;
            serve_mutation_disconnect_fixture(stream, instance_id).await
        });

        let locator = RuntimeLocator::for_testing_endpoint(instance_id, endpoint, "0.1.1");
        let mut runtime = locator
            .connect(ClientOptions::new("mutation fixture", "1.0.0"))
            .await
            .expect("connect mutation fixture");
        let result = runtime
            .sessions()
            .submit_input(&SubmitInputParams {
                request_id: request_id.clone(),
                session_id: RuntimeSessionId::new("session_fixture"),
                lease_id: "lease_fixture".to_owned(),
                lease_generation: 4,
                input: "unchanged fixture input".to_owned(),
            })
            .await;
        let ClientError::Runtime(error) = result.expect_err("transport loss is ambiguous") else {
            panic!("mutation transport loss returned the wrong failure kind");
        };
        assert_eq!(error.code, RuntimeErrorKind::OutcomeUnknown);
        assert_eq!(error.correlation_id, request_id.as_str());
        assert!(!error.retryable);

        let sent = server.await.expect("join mutation fixture");
        assert_eq!(sent.method, RuntimeMethod::SessionsSubmitInput.to_string());
        let sent: SubmitInputParams =
            serde_json::from_value(sent.params).expect("decode mutation parameters");
        assert_eq!(sent.request_id, request_id);
    }

    #[test]
    fn response_identifiers_and_json_rpc_version_are_exact() {
        let expected = JsonRpcId::Number(7);
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(7)).is_ok());
        assert!(validate_envelope("1.0", &expected, &JsonRpcId::Number(7)).is_err());
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(8)).is_err());
    }

    #[test]
    fn authenticated_initialization_accepts_only_a_current_or_newer_matching_grant() {
        let expected = IntegrationGrant {
            integration_id: runtrol_runtime_protocol::IntegrationId::new("int_fixture"),
            scopes: vec![AppScope::ProviderRead],
            roots: vec!["C:/work".to_owned()],
            key_generation: 2,
            grant_generation: 3,
        };
        let mut current = expected.clone();
        current.grant_generation = 4;
        current.scopes.push(AppScope::SessionList);
        assert!(initialization_grant_matches(
            Some(&current),
            Some(&expected)
        ));

        let mut wrong_key = current.clone();
        wrong_key.key_generation = 3;
        assert!(!initialization_grant_matches(
            Some(&wrong_key),
            Some(&expected)
        ));
        let mut same_generation_changed = expected.clone();
        same_generation_changed.roots.push("C:/other".to_owned());
        assert!(!initialization_grant_matches(
            Some(&same_generation_changed),
            Some(&expected)
        ));
    }

    #[test]
    fn initialization_signature_matches_the_language_neutral_fixture() {
        let identity = IntegrationIdentity::from_secret_bytes([7; 32]);
        let challenge = ServerChallenge {
            instance_id: "rtm_0123456789abcdef0123456789abcdef".to_owned(),
            nonce_id: "nonce_0123456789abcdef0123456789abcdef".to_owned(),
            nonce: Base64UrlUnpadded::encode_string(&[3; 32]),
            expires_at_ms: 2_000_000_000_000,
        };
        let client = ClientInfo {
            name: "fixture".to_owned(),
            version: "1.0.0".to_owned(),
        };
        let capabilities = ClientCapabilities::default();
        let authentication = IntegrationAuthentication {
            integration_id: runtrol_runtime_protocol::IntegrationId::new("int_fixture"),
            key_generation: 2,
            grant_generation: 3,
            signature: String::new(),
        };
        let payload = initialization_signing_payload(
            &challenge,
            &[runtrol_runtime_protocol::REVISION_2026_08_13],
            &client,
            &capabilities,
            &authentication,
        )
        .expect("canonical signing payload");
        assert_eq!(
            identity.sign_base64(&payload),
            "cBrwv1dkWz6oG-YszAimU6leDfkNriZSKxUNSGYttRiH2dD0RJQsTklzpjzW3_qSIZYwrPeSPLHnCyW5fJ5sBQ"
        );
    }

    #[test]
    fn challenge_validation_tolerates_bounded_local_clock_skew_and_nothing_beyond_it() {
        let now_ms = 2_000_000_000_000;
        let locator = ValidatedLocator {
            instance_id: "rtm_0123456789abcdef0123456789abcdef".to_owned(),
            endpoint: "fixture".to_owned(),
            runtime_version: "0.1.1".to_owned(),
        };
        let mut challenge = ServerChallenge {
            instance_id: locator.instance_id.clone(),
            nonce_id: "nonce_0123456789abcdef0123456789abcdef".to_owned(),
            nonce: Base64UrlUnpadded::encode_string(&[3; 32]),
            expires_at_ms: now_ms + CHALLENGE_LIFETIME_MS + CHALLENGE_CLOCK_SKEW_TOLERANCE_MS,
        };
        validate_challenge_at(&challenge, now_ms).expect("bounded local clock skew is accepted");

        challenge.expires_at_ms = challenge.expires_at_ms.saturating_add(1);
        let rejected = validate_challenge_at(&challenge, now_ms)
            .expect_err("clock skew beyond the bound is rejected");
        assert!(
            rejected
                .to_string()
                .contains("public lifetime and clock-skew bound")
        );
    }

    #[test]
    fn key_rotation_signature_matches_the_language_neutral_fixture() {
        let identity = IntegrationIdentity::from_secret_bytes([8; 32]);
        let mut params = RotateIntegrationKeyParams {
            request_id: "019c2b97-5f29-7b00-8000-000000000000"
                .parse()
                .expect("valid fixture mutation identity"),
            expected_key_generation: 2,
            new_public_key: identity.public_key_base64(),
            new_key_proof: String::new(),
        };
        let payload = key_rotation_signing_payload(
            &runtrol_runtime_protocol::IntegrationId::new("int_09090909090909090909090909090909"),
            3,
            &params,
        )
        .expect("canonical key rotation payload");
        params.new_key_proof = identity.sign_base64(&payload);
        assert_eq!(
            params.new_public_key,
            "E5j2LG0aRXxRumpLXz29L2n8qTIWIY3ImX5Ba9F9k8o"
        );
        assert_eq!(
            params.new_key_proof,
            "c3ZY8ElvUR3lVmFrkVrP5AnALg7q9bgcgU5DP0e0MhZZFaY_jGvRTEiesBUXnQyOjLepXGnx3xqkBmw-gZ_5CA"
        );
    }

    #[test]
    fn identity_round_trips_through_explicit_secret_storage() {
        let identity = IntegrationIdentity::from_secret_bytes([7; 32]);
        let restored = IntegrationIdentity::from_secret_bytes(identity.secret_bytes());
        assert_eq!(identity.public_key_base64(), restored.public_key_base64());
    }
}
