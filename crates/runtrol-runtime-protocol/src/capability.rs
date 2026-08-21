//! Provider-neutral structural capability observations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProviderId;

/// Select one provider for explicit capability discovery.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetProviderCapabilitiesParams {
    /// Opaque provider identity returned by `providers/list`.
    pub provider_id: ProviderId,
}

/// Whether one structural provider operation is usable in the observed installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapabilityAvailability {
    /// The exact prepared driver reports an available official surface.
    Available,
    /// No registered official surface exists.
    Unsupported,
    /// Provider negotiation is required before Runtime can answer.
    Unknown,
}

/// Provenance of an available structural capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapabilityProvenance {
    /// A provider-neutral protocol negotiation or stable protocol contract.
    OfficialProtocol,
    /// A provider-owned command, flag parser, or structured stream.
    OfficialCli,
    /// Runtime's registered driver lifecycle contract.
    DriverContract,
}

/// Freshness of a capability map relative to the installed binary identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityFreshness {
    /// The binary identity and driver observations were revalidated for this request.
    Current,
    /// A safe prior result is returned while a refresh is unavailable.
    Stale,
}

/// One sanitized structural capability observation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCapabilityObservation {
    /// Current availability.
    pub availability: ProviderCapabilityAvailability,
    /// Source of an available answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProviderCapabilityProvenance>,
    /// Stable explanation for an unsupported or unknown answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Structural lifecycle and event capabilities for one exact provider installation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeProviderCapabilities {
    /// Opaque provider identity selected by the caller.
    pub provider_id: ProviderId,
    /// Whether these observations match the current binary identity.
    pub freshness: CapabilityFreshness,
    /// A fresh session can be opened.
    pub fresh_session: ProviderCapabilityObservation,
    /// A provider-native session can be resumed.
    pub resume: ProviderCapabilityObservation,
    /// Provider output is mapped to structured Runtime events.
    pub structured_events: ProviderCapabilityObservation,
    /// A running turn can receive an interrupt request.
    pub interrupt: ProviderCapabilityObservation,
    /// Structured provider-native approvals can be answered.
    pub approvals: ProviderCapabilityObservation,
    /// A hot process can be released without deleting provider-native state.
    pub cooling: ProviderCapabilityObservation,
    /// An official provider-native session catalogue may be requested.
    pub native_session_catalogue: ProviderCapabilityObservation,
    /// The answering model can be switched mid-session.
    ///
    /// Optional for the same lockstep-additive reason as `waitingOn`: absent when an older Runtime
    /// answers, never guessed at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_model: Option<ProviderCapabilityObservation>,
    /// The reasoning effort can be switched mid-session.
    ///
    /// Distinct from the catalogue's per-model efforts: a CLI can offer an effort at open time and
    /// refuse to change it afterwards, and this is what lets a surface say "applies from the next
    /// session" before the attempt instead of after the refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_reasoning_effort: Option<ProviderCapabilityObservation>,
    /// A stored provider-native conversation can be deleted through the provider's own surface.
    ///
    /// Optional for the same lockstep-additive reason as the two above: absent when an older Runtime
    /// answers, never guessed at. A surface offers the act only where this says it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_delete: Option<ProviderCapabilityObservation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_unsupported_are_distinct_public_states() {
        let unknown = ProviderCapabilityObservation {
            availability: ProviderCapabilityAvailability::Unknown,
            provenance: None,
            why: Some("provider negotiation has not happened".to_owned()),
        };
        let unsupported = ProviderCapabilityObservation {
            availability: ProviderCapabilityAvailability::Unsupported,
            provenance: None,
            why: Some("no official surface exists".to_owned()),
        };
        assert_ne!(unknown, unsupported);
        assert_eq!(
            serde_json::to_value(unknown).expect("serialize capability"),
            serde_json::json!({
                "availability": "unknown",
                "why": "provider negotiation has not happened"
            })
        );
    }
}
