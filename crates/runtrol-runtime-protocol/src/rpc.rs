//! JSON-RPC envelopes and initialization DTOs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    IntegrationAuthentication, IntegrationGrant, MAX_FRAME_BYTES, MAX_INPUT_BYTES, MAX_PAGE_ITEMS,
    MAX_SUBSCRIPTIONS, ProtocolRevision, RuntimeError,
};

/// A JSON-RPC request identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Connection-local numeric identifier.
    Number(u64),
    /// Caller-owned opaque string identifier.
    String(String),
}

/// One JSON-RPC request. Method parameter schemas remain closed at their typed decode boundary.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    /// Must be exactly `2.0`.
    pub jsonrpc: String,
    /// Connection-local response correlation.
    pub id: JsonRpcId,
    /// Stable public method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
}

/// One JSON-RPC notification with no response ID.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcNotification {
    /// Must be exactly `2.0`.
    pub jsonrpc: String,
    /// Stable public method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
}

/// Successful JSON-RPC response.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessResponse {
    /// Must be exactly `2.0`.
    pub jsonrpc: String,
    /// Request identifier copied unchanged.
    pub id: JsonRpcId,
    /// Typed method result represented at the envelope boundary.
    pub result: Value,
}

/// Failed JSON-RPC response.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// Must be exactly `2.0`.
    pub jsonrpc: String,
    /// Request identifier copied unchanged.
    pub id: JsonRpcId,
    /// Stable public failure.
    pub error: RuntimeError,
}

/// A response is exactly one success or one failure.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    /// A typed result encoded in the envelope.
    Success(SuccessResponse),
    /// A stable machine failure.
    Error(ErrorResponse),
}

/// Safe client presentation metadata.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientInfo {
    /// Product display name.
    pub name: String,
    /// Product version text.
    pub version: String,
}

/// Client features understood by the initial read-only revision.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCapabilities {
    /// Whether the client preserves bounded unknown optional event extensions.
    #[serde(default)]
    pub opaque_event_extensions: bool,
}

/// Initialization is negotiation only. Inventory is a separate authorized request.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeParams {
    /// Every finalized revision the client implements.
    pub supported_revisions: Vec<ProtocolRevision>,
    /// Safe client metadata.
    pub client: ClientInfo,
    /// Closed client capability map.
    #[serde(default)]
    pub client_capabilities: ClientCapabilities,
    /// Approved integration proof when reconnecting. Omit only to enroll.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<IntegrationAuthentication>,
}

/// Public Runtime instance facts used to reject a stale or replaced locator.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeInstance {
    /// Changes when a newly installed Runtime home is created.
    pub instance_id: String,
    /// Product version, not the compatibility decision.
    pub version: String,
    /// Target class of the running artifact.
    pub platform: String,
}

/// Public product capabilities for the selected revision.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the negotiated public wire needs independent closed capability flags"
)]
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCapabilities {
    /// Local integration enrollment is implemented.
    pub integration_enrollment: bool,
    /// Fast provider inventory method is implemented.
    pub provider_inventory: bool,
    /// Fast managed session snapshot is implemented.
    pub managed_session_list: bool,
    /// Explicit provider model discovery is implemented.
    pub model_discovery: bool,
    /// Root-scoped official provider-native session discovery is implemented.
    pub native_session_catalogue: bool,
    /// Renewable single-writer session control is implemented.
    pub session_control: bool,
    /// Bounded replay followed by live normalized events is implemented.
    pub session_events: bool,
}

/// Numeric public bounds advertised during initialization.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeLimits {
    /// Maximum frame payload before allocation.
    pub max_frame_bytes: usize,
    /// Maximum caller input bytes.
    pub max_input_bytes: usize,
    /// Maximum opaque model selection bytes on session start.
    pub max_model_selection_bytes: usize,
    /// Maximum authenticated native adoption token bytes.
    pub max_native_adoption_token_bytes: usize,
    /// Maximum wrapped native catalogue cursor bytes.
    pub max_native_public_cursor_bytes: usize,
    /// Maximum items in one catalogue page.
    pub max_page_items: u16,
    /// Maximum subscriptions on one connection.
    pub max_subscriptions: u16,
    /// Lifetime of one server-first challenge.
    pub challenge_lifetime_ms: u64,
    /// Lifetime of one pending local integration enrollment.
    pub enrollment_lifetime_ms: u64,
    /// Maximum active pending integration enrollments.
    pub max_pending_enrollments: u16,
    /// Maximum finalized protocol revisions accepted in one offer.
    pub max_revision_offers: u16,
    /// Lifetime of one renewable control lease.
    pub control_lease_lifetime_ms: u64,
    /// Maximum age of a mutation identity with a known outcome.
    pub idempotency_window_ms: u64,
    /// Maximum retained mutation identities across integrations.
    pub max_idempotency_records: u16,
    /// Lifetime of native catalogue cursors and adoption observations.
    pub native_cursor_lifetime_ms: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: MAX_FRAME_BYTES,
            max_input_bytes: MAX_INPUT_BYTES,
            max_model_selection_bytes: crate::MAX_MODEL_SELECTION_BYTES,
            max_native_adoption_token_bytes: crate::MAX_NATIVE_ADOPTION_TOKEN_BYTES,
            max_native_public_cursor_bytes: crate::MAX_NATIVE_PUBLIC_CURSOR_BYTES,
            max_page_items: MAX_PAGE_ITEMS,
            max_subscriptions: MAX_SUBSCRIPTIONS,
            challenge_lifetime_ms: crate::CHALLENGE_LIFETIME_MS,
            enrollment_lifetime_ms: crate::ENROLLMENT_LIFETIME_MS,
            max_pending_enrollments: crate::MAX_PENDING_ENROLLMENTS,
            max_revision_offers: crate::MAX_REVISION_OFFERS,
            control_lease_lifetime_ms: crate::CONTROL_LEASE_LIFETIME_MS,
            idempotency_window_ms: crate::IDEMPOTENCY_WINDOW_MS,
            max_idempotency_records: crate::MAX_IDEMPOTENCY_RECORDS,
            native_cursor_lifetime_ms: crate::NATIVE_CURSOR_LIFETIME_MS,
        }
    }
}

/// Successful initialization before integration authorization.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeResult {
    /// Newest common finalized revision.
    pub selected_revision: ProtocolRevision,
    /// Running instance proof.
    pub runtime: RuntimeInstance,
    /// Implemented product capabilities. Scope checks still apply.
    pub server_capabilities: RuntimeCapabilities,
    /// Numeric admission limits.
    pub limits: RuntimeLimits,
    /// Current authority when initialization authenticated an approved integration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<IntegrationGrant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::revision::REVISION_2026_08_13;

    #[test]
    fn closed_initialization_rejects_unknown_security_shaped_fields() {
        let json = r#"{
            "supportedRevisions":["2026-08-13"],
            "client":{"name":"fixture","version":"1.0.0"},
            "clientCapabilities":{},
            "scope":"session.input.write"
        }"#;
        assert!(serde_json::from_str::<InitializeParams>(json).is_err());
    }

    #[test]
    fn default_limits_are_the_public_constants() {
        let limits = RuntimeLimits::default();
        assert_eq!(limits.max_frame_bytes, MAX_FRAME_BYTES);
        assert_eq!(limits.max_input_bytes, MAX_INPUT_BYTES);
        assert_eq!(
            limits.max_model_selection_bytes,
            crate::MAX_MODEL_SELECTION_BYTES
        );
        assert_eq!(
            limits.max_native_adoption_token_bytes,
            crate::MAX_NATIVE_ADOPTION_TOKEN_BYTES
        );
        assert_eq!(
            limits.max_native_public_cursor_bytes,
            crate::MAX_NATIVE_PUBLIC_CURSOR_BYTES
        );
        assert_eq!(limits.max_page_items, MAX_PAGE_ITEMS);
        assert_eq!(limits.max_subscriptions, MAX_SUBSCRIPTIONS);
        assert_eq!(limits.challenge_lifetime_ms, crate::CHALLENGE_LIFETIME_MS);
        assert_eq!(limits.enrollment_lifetime_ms, crate::ENROLLMENT_LIFETIME_MS);
        assert_eq!(
            limits.max_pending_enrollments,
            crate::MAX_PENDING_ENROLLMENTS
        );
        assert_eq!(limits.max_revision_offers, crate::MAX_REVISION_OFFERS);
        assert_eq!(
            limits.control_lease_lifetime_ms,
            crate::CONTROL_LEASE_LIFETIME_MS
        );
        assert_eq!(limits.idempotency_window_ms, crate::IDEMPOTENCY_WINDOW_MS);
        assert_eq!(
            limits.max_idempotency_records,
            crate::MAX_IDEMPOTENCY_RECORDS
        );
        assert_eq!(
            limits.native_cursor_lifetime_ms,
            crate::NATIVE_CURSOR_LIFETIME_MS
        );
    }

    #[test]
    fn initialization_round_trips_without_an_inventory_side_channel() {
        let params = InitializeParams {
            supported_revisions: vec![REVISION_2026_08_13],
            client: ClientInfo {
                name: "fixture".to_owned(),
                version: "1.0.0".to_owned(),
            },
            client_capabilities: ClientCapabilities::default(),
            authentication: None,
        };
        let value = serde_json::to_value(&params).expect("serializable");
        assert!(value.get("providers").is_none());
        let decoded = serde_json::from_value::<InitializeParams>(value).expect("deserializable");
        assert_eq!(decoded, params);
    }
}
