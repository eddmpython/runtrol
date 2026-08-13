//! Typed initialization, enrollment, and read-only Runtime operation groups.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, CHALLENGE_LIFETIME_MS,
    ClientCapabilities, ClientInfo, ControlLease, ControlLeaseParams, EnrollmentDecision,
    EnrollmentManifest, EnrollmentReceipt, ErrorResponse, FINALIZED_REVISIONS,
    GetProviderCapabilitiesParams, GetSessionParams, InitializeParams, InitializeResult,
    IntegrationAuthentication, IntegrationGrant, JsonRpcId, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LaggedNotification, ListModelsParams, ListNativeSessionsParams,
    ManagedSessionList, NativeSessionCatalogue, PendingEnrollmentId, ProviderId, ProviderList,
    RequestEnrollmentParams, ResumeSessionParams, RuntimeEventNotification, RuntimeMethod,
    RuntimeModelCatalog, RuntimeProviderCapabilities, RuntimeSessionId, ServerChallenge,
    SessionDescriptor, SessionOpenResult, StartSessionParams, SubmitInputParams, SuccessResponse,
    WatchEnrollmentParams, WatchEventsParams, WatchEventsResult, enrollment_signing_payload,
    initialization_signing_payload,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ClientError;
use crate::connection::Connection;
use crate::identity::{IntegrationCredentials, IntegrationIdentity};
use crate::locator::{LocatorState, RuntimeLocator, ValidatedLocator};

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
    if challenge.expires_at_ms <= now_ms {
        return Err(ClientError::Protocol(
            "the Runtime challenge is already expired".to_owned(),
        ));
    }
    if challenge.expires_at_ms > now_ms.saturating_add(CHALLENGE_LIFETIME_MS) {
        return Err(ClientError::Protocol(
            "the Runtime challenge exceeds the public lifetime bound".to_owned(),
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
    if initialized.grant.as_ref() != expected_grant {
        return Err(ClientError::Protocol(
            "the Runtime initialization grant does not match the authenticated credentials"
                .to_owned(),
        ));
    }
    Ok(())
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
            .call(RuntimeMethod::SessionsStart, params)
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
            .call(RuntimeMethod::SessionsAdoptNative, params)
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
            .call(RuntimeMethod::SessionsResume, params)
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
            .call(RuntimeMethod::SessionsAcquireControl, params)
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
            .call(RuntimeMethod::SessionsRenewControl, params)
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
            .call(RuntimeMethod::SessionsReleaseControl, params)
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
            .call(RuntimeMethod::SessionsSubmitInput, params)
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
            .call(RuntimeMethod::SessionsInterrupt, params)
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
        let payload = self.runtime.connection.receive().await?;
        let notification: JsonRpcNotification =
            serde_json::from_slice(&payload).map_err(|error| {
                ClientError::Protocol(format!(
                    "session notification is not valid public JSON-RPC: {error}"
                ))
            })?;
        if notification.jsonrpc != "2.0" {
            return Err(ClientError::Protocol(
                "session notification JSON-RPC version is not 2.0".to_owned(),
            ));
        }
        let method = notification.method.parse::<RuntimeMethod>().map_err(|_| {
            ClientError::Protocol("session notification method is unknown".to_owned())
        })?;
        match method {
            RuntimeMethod::SessionsEvent => {
                let event: RuntimeEventNotification = serde_json::from_value(notification.params)
                    .map_err(|error| {
                    ClientError::Protocol(format!(
                        "session event notification has the wrong shape: {error}"
                    ))
                })?;
                self.validate_target(&event.subscription_id, &event.session_id)?;
                Ok(SessionNotification::Event(event))
            }
            RuntimeMethod::SessionsLagged => {
                let lagged: LaggedNotification = serde_json::from_value(notification.params)
                    .map_err(|error| {
                        ClientError::Protocol(format!(
                            "session lag notification has the wrong shape: {error}"
                        ))
                    })?;
                self.validate_target(&lagged.subscription_id, &lagged.session_id)?;
                Ok(SessionNotification::Lagged(lagged))
            }
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
            | RuntimeMethod::SessionsAcquireControl
            | RuntimeMethod::SessionsRenewControl
            | RuntimeMethod::SessionsReleaseControl
            | RuntimeMethod::SessionsSubmitInput
            | RuntimeMethod::SessionsWatchEvents
            | RuntimeMethod::SessionsInterrupt
            | RuntimeMethod::PanicStop => Err(ClientError::Protocol(
                "the dedicated session stream received a non-event method".to_owned(),
            )),
        }
    }

    fn validate_target(
        &self,
        subscription_id: &str,
        session_id: &RuntimeSessionId,
    ) -> Result<(), ClientError> {
        if subscription_id != self.subscription_id || session_id != &self.session_id {
            return Err(ClientError::Protocol(
                "session notification target does not match its subscription".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One item on a dedicated public session stream.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionNotification {
    /// One normalized provider-neutral event and its next reconnect boundary.
    Event(RuntimeEventNotification),
    /// The subscriber fell behind the bounded queue and must reconnect from the named boundary.
    Lagged(LaggedNotification),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_options_contain_no_endpoint_or_provider_choice() {
        let options = ClientOptions::new("fixture", "1.0.0");
        assert_eq!(options.name, "fixture");
        assert_eq!(options.version, "1.0.0");
    }

    #[test]
    fn response_identifiers_and_json_rpc_version_are_exact() {
        let expected = JsonRpcId::Number(7);
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(7)).is_ok());
        assert!(validate_envelope("1.0", &expected, &JsonRpcId::Number(7)).is_err());
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(8)).is_err());
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
    fn identity_round_trips_through_explicit_secret_storage() {
        let identity = IntegrationIdentity::from_secret_bytes([7; 32]);
        let restored = IntegrationIdentity::from_secret_bytes(identity.secret_bytes());
        assert_eq!(identity.public_key_base64(), restored.public_key_base64());
    }
}
