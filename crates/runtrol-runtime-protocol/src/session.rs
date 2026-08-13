//! Public Runtime control leases, mutation identities, and bounded event subscription DTOs.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{LifecycleState, ProtocolRevision, RuntimeSessionId};

/// Maximum lifetime of one renewable session control lease.
pub const CONTROL_LEASE_LIFETIME_MS: u64 = 30_000;

/// Maximum future clock skew accepted for a caller-minted mutation identity.
pub const MUTATION_CLOCK_SKEW_MS: u64 = 5 * 60_000;

/// How long Runtime remembers mutation outcomes and refuses stale identities.
pub const IDEMPOTENCY_WINDOW_MS: u64 = 24 * 60 * 60_000;

/// Maximum retained mutation identities across integrations.
pub const MAX_IDEMPOTENCY_RECORDS: u16 = 2_048;

/// A caller-minted UUIDv7 identifying one state-changing request.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct MutationRequestId(String);

impl MutationRequestId {
    /// Mint a new time-ordered mutation identity.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    /// The canonical lowercase UUID spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Unix millisecond timestamp embedded in the UUIDv7 identity.
    #[must_use]
    pub fn unix_millis(&self) -> Option<u64> {
        let Ok(uuid) = Uuid::parse_str(&self.0) else {
            return None;
        };
        let (seconds, nanos) = uuid.get_timestamp()?.to_unix();
        seconds
            .checked_mul(1_000)?
            .checked_add(u64::from(nanos / 1_000_000))
    }

    /// The exact UUID bytes used as part of the durable per-integration key.
    #[must_use]
    pub fn to_bytes(&self) -> Option<[u8; 16]> {
        match Uuid::parse_str(&self.0) {
            Ok(uuid) => Some(*uuid.as_bytes()),
            Err(_) => None,
        }
    }
}

impl FromStr for MutationRequestId {
    type Err = MutationRequestIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(value).map_err(|_| MutationRequestIdError)?;
        if uuid.get_version_num() != 7 || uuid.hyphenated().to_string() != value {
            return Err(MutationRequestIdError);
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for MutationRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for MutationRequestId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MutationRequestId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A mutation identity is not a canonical lowercase UUIDv7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("mutation request identity must be a canonical lowercase UUIDv7")]
pub struct MutationRequestIdError;

/// One opaque renewable control authority for one live session incarnation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlLease {
    /// Opaque unguessable lease identity.
    pub lease_id: String,
    /// Exact controlled session.
    pub session_id: RuntimeSessionId,
    /// Session lifecycle generation observed when the lease was acquired.
    pub session_generation: u64,
    /// Monotonic lease generation required on renewal and mutations.
    pub lease_generation: u64,
    /// Wall-clock expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Acquire control only if the caller still sees this exact live state.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireControlParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Lifecycle state visible when the user chose the action.
    pub expected_lifecycle: LifecycleState,
    /// Lifecycle generation visible when the user chose the action.
    pub expected_session_generation: u64,
}

/// Renew or release one exact lease generation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlLeaseParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
}

/// Submit caller-owned input under one exact control lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitInputParams {
    /// Caller-minted idempotency identity.
    pub request_id: MutationRequestId,
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// Caller-owned input transported without prefix, suffix, or rewriting.
    pub input: String,
}

/// Interrupt one exact controlled session.
pub type InterruptParams = ControlLeaseParams;

/// Cool one exact idle session while retaining its provider-native pointer.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoolSessionParams {
    /// Caller-minted idempotency identity.
    pub request_id: MutationRequestId,
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Lifecycle generation visible when the user chose to cool the session.
    pub expected_session_generation: u64,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
}

/// Public reconnect cursor over the existing bounded Runtime replay ring.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventCursor {
    /// Runtime-minted stream identity.
    pub stream: String,
    /// Live process attachment generation.
    pub epoch: u32,
    /// Dense next event sequence.
    pub seq: u64,
}

/// Explicit replay gap when the requested cursor fell outside the bounded ring.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventGap {
    /// Cursor the consumer requested.
    pub requested: EventCursor,
    /// Oldest boundary Runtime can serve now.
    pub live_at: EventCursor,
}

/// Install one bounded event subscription on a dedicated connection.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchEventsParams {
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Next expected event, or no cursor for the current bounded tail.
    pub after: Option<EventCursor>,
}

/// Event subscription boundary returned before replay or live delivery.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchEventsResult {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Exact watched session.
    pub session_id: RuntimeSessionId,
    /// First delivered boundary.
    pub starts_at: EventCursor,
    /// Boundary between replay and newly published events.
    pub live_at: EventCursor,
    /// Explicit bounded replay miss.
    pub gap: Option<EventGap>,
}

/// One provider-neutral normalized event notification.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEventNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Exact watched session.
    pub session_id: RuntimeSessionId,
    /// Public event vocabulary revision.
    pub event_revision: ProtocolRevision,
    /// Existing normalized structural event, transported without interpretation by Runtime.
    pub event: Value,
    /// Next exact reconnect boundary.
    pub next_expected: EventCursor,
}

/// A slow subscriber was retired at an exact missing boundary.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaggedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Exact watched session.
    pub session_id: RuntimeSessionId,
    /// First event the subscriber did not receive.
    pub next_expected: EventCursor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_identity_is_exact_uuid_v7() {
        let id = MutationRequestId::now();
        assert_eq!(id.as_str().parse(), Ok(id.clone()));
        assert!(
            "550e8400-e29b-41d4-a716-446655440000"
                .parse::<MutationRequestId>()
                .is_err()
        );
        assert!(
            id.as_str()
                .to_uppercase()
                .parse::<MutationRequestId>()
                .is_err()
        );
    }
}
