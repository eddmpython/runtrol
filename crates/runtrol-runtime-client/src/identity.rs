//! Consumer-owned signing identity and approved reconnect credentials.

use base64ct::{Base64UrlUnpadded, Encoding as _};
use ed25519_dalek::{Signer as _, SigningKey};
use runtrol_runtime_protocol::{
    IntegrationGrant, PendingEnrollmentId, self_approval_signing_payload,
};

use crate::ClientError;

/// One consumer-owned Ed25519 identity. Runtime receives only its public key.
#[derive(Clone)]
pub struct IntegrationIdentity {
    signing: SigningKey,
}

impl core::fmt::Debug for IntegrationIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("IntegrationIdentity([redacted])")
    }
}

impl IntegrationIdentity {
    /// Generate a new installed-integration identity.
    ///
    /// # Errors
    ///
    /// The operating system random source was unavailable.
    pub fn generate() -> Result<Self, ClientError> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|error| {
            ClientError::Protocol(format!(
                "the integration identity could not be generated: {error}"
            ))
        })?;
        Ok(Self::from_secret_bytes(secret))
    }

    /// Restore an identity from consumer-owned secure storage.
    #[must_use]
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&secret),
        }
    }

    /// Export bytes for the consumer's own operating-system secure storage.
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// Base64url public verification key used in an enrollment manifest.
    #[must_use]
    pub fn public_key_base64(&self) -> String {
        Base64UrlUnpadded::encode_string(self.signing.verifying_key().as_bytes())
    }

    /// Sign the one pending enrollment identity for a local self-approval request.
    ///
    /// This deliberately exposes only the domain-separated self-approval proof rather than a general signing
    /// primitive. The returned signature cannot authenticate a Runtime connection or another enrollment.
    ///
    /// # Errors
    ///
    /// Canonical protocol serialization failed, which indicates an implementation defect.
    pub fn self_approval_signature(
        &self,
        pending_id: &PendingEnrollmentId,
    ) -> Result<String, ClientError> {
        let payload = self_approval_signing_payload(pending_id).map_err(|error| {
            ClientError::Protocol(format!(
                "the self-approval proof payload cannot be encoded: {error}"
            ))
        })?;
        Ok(self.sign_base64(&payload))
    }

    pub(crate) fn sign_base64(&self, payload: &[u8]) -> String {
        Base64UrlUnpadded::encode_string(&self.signing.sign(payload).to_bytes())
    }
}

/// Consumer-owned identity paired with the current approved public grant.
#[derive(Clone, Debug)]
pub struct IntegrationCredentials {
    identity: IntegrationIdentity,
    grant: IntegrationGrant,
}

impl IntegrationCredentials {
    /// Bind the locally stored private identity to a grant returned for that identity.
    #[must_use]
    pub const fn new(identity: IntegrationIdentity, grant: IntegrationGrant) -> Self {
        Self { identity, grant }
    }

    /// Consumer-owned signing identity.
    #[must_use]
    pub const fn identity(&self) -> &IntegrationIdentity {
        &self.identity
    }

    /// Current reconnect grant and generations.
    #[must_use]
    pub const fn grant(&self) -> &IntegrationGrant {
        &self.grant
    }

    pub(crate) fn into_parts(self) -> (IntegrationIdentity, IntegrationGrant) {
        (self.identity, self.grant)
    }
}
