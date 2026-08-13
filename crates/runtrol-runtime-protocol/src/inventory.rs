//! Provider-neutral read-only inventory DTOs.

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

            /// The opaque text for transport and equality only.
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
    ProviderId,
    "An opaque provider identity discovered by Runtime."
);
opaque_id!(
    RuntimeSessionId,
    "A stable Runtime-managed session identity."
);

/// Whether an installed provider can currently be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationState {
    /// The registered discovery ladder found a usable program.
    Usable,
    /// No program matching the registered discovery ladder was found.
    Missing,
    /// A program was found but its supported surface could not be confirmed.
    Unavailable,
}

/// Structural provider installation evidence with no credentials or raw output.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationObservation {
    /// Current usable state.
    pub state: InstallationState,
    /// Provider-owned version text when the bounded probe supplied it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Safe structural reason when the provider is not usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One provider in the fast inventory.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDescriptor {
    /// Opaque selection value.
    pub provider_id: ProviderId,
    /// Provider or manifest supplied label for presentation only.
    pub display_name: String,
    /// Fast cached installation evidence. Listing does not start the provider.
    pub installation: InstallationObservation,
}

/// A bounded provider inventory snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderList {
    /// Providers in registry order, including unavailable entries.
    pub providers: Vec<ProviderDescriptor>,
}

/// Structural Runtime supervision state without conversation meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleState {
    /// A provider process is present and not currently in a turn.
    HotIdle,
    /// A provider process is in a turn.
    HotRunning,
    /// Runtime has metadata but no provider process.
    Cold,
    /// The last provider operation failed structurally.
    Failed,
}

/// One Runtime-managed session in the immediate catalogue.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDescriptor {
    /// Stable Runtime selection value.
    pub session_id: RuntimeSessionId,
    /// Opaque provider selection value.
    pub provider_id: ProviderId,
    /// Current supervision state.
    pub lifecycle: LifecycleState,
    /// Operator-owned label, never derived from conversation content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A bounded Runtime-managed session snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSessionList {
    /// Sessions already known to Runtime.
    pub sessions: Vec<SessionDescriptor>,
    /// Safe structural omissions that do not identify an unauthorized project.
    pub warnings: Vec<String>,
}
