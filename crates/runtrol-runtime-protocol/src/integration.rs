//! Integration enrollment, authentication, grants, and stable app scopes.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ClientCapabilities, ClientInfo, MutationRequestId, ProtocolRevision};

macro_rules! opaque_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct an opaque identifier after its owning boundary validated it.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Opaque transport text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

opaque_id!(
    IntegrationId,
    "An operator-approved local integration instance."
);
opaque_id!(
    PendingEnrollmentId,
    "An opaque pending local enrollment decision."
);

/// Public integration authority, separate from remote device scopes.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub enum AppScope {
    /// Read provider descriptors and structural capabilities.
    #[serde(rename = "provider.read")]
    ProviderRead,
    /// Read official model catalogues.
    #[serde(rename = "model.read")]
    ModelRead,
    /// List managed session metadata under approved roots.
    #[serde(rename = "session.list")]
    SessionList,
    /// Read official native-session catalogues.
    #[serde(rename = "session.native.discover")]
    SessionNativeDiscover,
    /// Read bounded live normalized events.
    #[serde(rename = "session.output.read")]
    SessionOutputRead,
    /// Start sessions under approved roots.
    #[serde(rename = "session.start")]
    SessionStart,
    /// Resume or adopt sessions under approved roots.
    #[serde(rename = "session.resume")]
    SessionResume,
    /// Acquire control and submit caller-owned input.
    #[serde(rename = "session.input.write")]
    SessionInputWrite,
    /// Interrupt or cool one controlled session.
    #[serde(rename = "session.stop")]
    SessionStop,
    /// Answer exact low-risk structured approvals.
    #[serde(rename = "approval.respond.low")]
    ApprovalRespondLow,
    /// Answer supported high-risk approvals under additional local policy.
    #[serde(rename = "approval.respond.high")]
    ApprovalRespondHigh,
    /// Forget a Runtime pointer without deleting provider state.
    #[serde(rename = "session.delete")]
    SessionDelete,
}

impl AppScope {
    /// Every scope in stable presentation order.
    pub const ALL: &'static [Self] = &[
        Self::ProviderRead,
        Self::ModelRead,
        Self::SessionList,
        Self::SessionNativeDiscover,
        Self::SessionOutputRead,
        Self::SessionStart,
        Self::SessionResume,
        Self::SessionInputWrite,
        Self::SessionStop,
        Self::ApprovalRespondLow,
        Self::ApprovalRespondHigh,
        Self::SessionDelete,
    ];

    /// Stable wire and durable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderRead => "provider.read",
            Self::ModelRead => "model.read",
            Self::SessionList => "session.list",
            Self::SessionNativeDiscover => "session.native.discover",
            Self::SessionOutputRead => "session.output.read",
            Self::SessionStart => "session.start",
            Self::SessionResume => "session.resume",
            Self::SessionInputWrite => "session.input.write",
            Self::SessionStop => "session.stop",
            Self::ApprovalRespondLow => "approval.respond.low",
            Self::ApprovalRespondHigh => "approval.respond.high",
            Self::SessionDelete => "session.delete",
        }
    }
}

impl fmt::Display for AppScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AppScope {
    type Err = UnknownAppScope;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|scope| scope.as_str() == value)
            .copied()
            .ok_or_else(|| UnknownAppScope(value.to_owned()))
    }
}

/// A stored or supplied scope is outside the finalized vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown Runtime app scope {0:?}")]
pub struct UnknownAppScope(String);

/// One connection-bound challenge sent before initialization.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerChallenge {
    /// Runtime instance named by the verified locator.
    pub instance_id: String,
    /// Opaque one-use nonce identity.
    pub nonce_id: String,
    /// Base64url random challenge bytes.
    pub nonce: String,
    /// Wall-clock expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Authentication for an approved integration during initialization.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationAuthentication {
    /// Approved integration.
    pub integration_id: IntegrationId,
    /// Public key generation expected by the client.
    pub key_generation: u64,
    /// Grant generation expected by the client.
    pub grant_generation: u64,
    /// Base64url Ed25519 signature over the canonical initialization payload.
    pub signature: String,
}

/// Closed self-description and exact authority requested for first enrollment.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentManifest {
    /// Consumer-minted installed-instance label, never authority by itself.
    pub client_instance_id: String,
    /// Base64url Ed25519 verifying key.
    pub public_key: String,
    /// Base64url digest of the consumer's closed product manifest.
    pub manifest_digest: String,
    /// Exact requested scopes. Approval may only narrow them.
    pub requested_scopes: Vec<AppScope>,
    /// Exact requested project paths. Approval canonicalizes and may only narrow them.
    pub requested_roots: Vec<String>,
}

/// Prove possession of the key attached to a new enrollment.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEnrollmentParams {
    /// Closed proposal.
    pub manifest: EnrollmentManifest,
    /// Base64url Ed25519 signature over the canonical enrollment payload.
    pub signature: String,
}

/// Enrollment was recorded without revealing Runtime inventory.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentReceipt {
    /// Opaque local decision identity.
    pub pending_id: PendingEnrollmentId,
    /// When the request expires in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Read one pending decision on the same proved connection.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchEnrollmentParams {
    /// Pending decision returned for this key.
    pub pending_id: PendingEnrollmentId,
}

/// Current enrollment decision.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum EnrollmentDecision {
    /// Local approval has not happened yet.
    Pending,
    /// Exact narrowed grant was approved.
    Approved {
        /// Grant ready for authenticated reconnect.
        grant: IntegrationGrant,
    },
    /// The local operator denied the request.
    Denied,
    /// The bounded decision window elapsed.
    Expired,
}

/// The caller's current approved authority.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationGrant {
    /// Stable integration identity.
    pub integration_id: IntegrationId,
    /// Exact current app scopes.
    pub scopes: Vec<AppScope>,
    /// Canonical approved project paths.
    pub roots: Vec<String>,
    /// Key generation required for signatures.
    pub key_generation: u64,
    /// Generation invalidating already parsed requests after grant changes.
    pub grant_generation: u64,
}

/// Replace one approved integration key after an exact local confirmation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RotateIntegrationKeyParams {
    /// Caller-owned idempotency identity retained across retries.
    pub request_id: MutationRequestId,
    /// Key generation observed before the rotation began.
    pub expected_key_generation: u64,
    /// Base64url Ed25519 verification key that will replace the current key.
    pub new_public_key: String,
    /// Base64url signature by the new key over the canonical rotation payload.
    pub new_key_proof: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializationSigningPayload<'a> {
    domain: &'static str,
    challenge: &'a ServerChallenge,
    supported_revisions: &'a [ProtocolRevision],
    client: &'a ClientInfo,
    client_capabilities: &'a ClientCapabilities,
    integration_id: &'a IntegrationId,
    key_generation: u64,
    grant_generation: u64,
}

/// Canonical bytes signed by an approved integration during initialization.
///
/// # Errors
///
/// Serialization failure, which indicates a protocol implementation defect rather than caller data.
pub fn initialization_signing_payload(
    challenge: &ServerChallenge,
    supported_revisions: &[ProtocolRevision],
    client: &ClientInfo,
    capabilities: &ClientCapabilities,
    authentication: &IntegrationAuthentication,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&InitializationSigningPayload {
        domain: "runtrol-runtime-initialize-v1",
        challenge,
        supported_revisions,
        client,
        client_capabilities: capabilities,
        integration_id: &authentication.integration_id,
        key_generation: authentication.key_generation,
        grant_generation: authentication.grant_generation,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentSigningPayload<'a> {
    domain: &'static str,
    challenge: &'a ServerChallenge,
    supported_revisions: &'a [ProtocolRevision],
    selected_revision: ProtocolRevision,
    client: &'a ClientInfo,
    client_capabilities: &'a ClientCapabilities,
    manifest: &'a EnrollmentManifest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyRotationSigningPayload<'a> {
    domain: &'static str,
    integration_id: &'a IntegrationId,
    grant_generation: u64,
    request_id: &'a MutationRequestId,
    expected_key_generation: u64,
    new_public_key: &'a str,
}

/// Canonical bytes proving possession of the key proposed for enrollment.
///
/// # Errors
///
/// Serialization failure, which indicates a protocol implementation defect rather than caller data.
pub fn enrollment_signing_payload(
    challenge: &ServerChallenge,
    supported_revisions: &[ProtocolRevision],
    selected_revision: ProtocolRevision,
    client: &ClientInfo,
    capabilities: &ClientCapabilities,
    manifest: &EnrollmentManifest,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&EnrollmentSigningPayload {
        domain: "runtrol-runtime-enrollment-v1",
        challenge,
        supported_revisions,
        selected_revision,
        client,
        client_capabilities: capabilities,
        manifest,
    })
}

/// Canonical bytes proving possession of a proposed replacement key.
///
/// # Errors
///
/// Serialization failure, which indicates a protocol implementation defect rather than caller data.
pub fn key_rotation_signing_payload(
    integration_id: &IntegrationId,
    grant_generation: u64,
    params: &RotateIntegrationKeyParams,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&KeyRotationSigningPayload {
        domain: "runtrol-runtime-key-rotation-v1",
        integration_id,
        grant_generation,
        request_id: &params.request_id,
        expected_key_generation: params.expected_key_generation,
        new_public_key: &params.new_public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_scope_names_are_unique_and_round_trip() {
        let mut names = std::collections::BTreeSet::new();
        for scope in AppScope::ALL {
            assert!(names.insert(scope.as_str()));
            assert_eq!(scope.as_str().parse(), Ok(*scope));
        }
    }

    #[test]
    fn enrollment_proof_binds_the_exact_revision_offer() {
        let challenge = ServerChallenge {
            instance_id: "instance".to_owned(),
            nonce_id: "nonce_fixture".to_owned(),
            nonce: "fixture".to_owned(),
            expires_at_ms: 1,
        };
        let selected = crate::REVISION_2026_08_13;
        let older = ProtocolRevision::new(2026, 1, 1);
        let client = ClientInfo {
            name: "fixture".to_owned(),
            version: "1.0.0".to_owned(),
        };
        let capabilities = ClientCapabilities::default();
        let manifest = EnrollmentManifest {
            client_instance_id: "fixture-instance".to_owned(),
            public_key: "key".to_owned(),
            manifest_digest: "digest".to_owned(),
            requested_scopes: vec![AppScope::ProviderRead],
            requested_roots: Vec::new(),
        };
        let exact = enrollment_signing_payload(
            &challenge,
            &[selected],
            selected,
            &client,
            &capabilities,
            &manifest,
        )
        .expect("payload");
        let altered = enrollment_signing_payload(
            &challenge,
            &[older, selected],
            selected,
            &client,
            &capabilities,
            &manifest,
        )
        .expect("payload");
        assert_ne!(exact, altered);
    }
}
