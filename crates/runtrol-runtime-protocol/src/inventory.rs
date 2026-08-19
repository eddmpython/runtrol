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

/// A coding service's own commands for making itself usable, ready to show a person.
///
/// # Why Runtime sends finished command lines
///
/// A declaration names arguments; only Runtime knows which executable actually resolved. A client that
/// joined the two would be a second place that decides what runs, and it would be wrong on exactly the
/// machine where a second candidate was the installed one.
///
/// # What a client may do with these
///
/// Offer them. Nothing else. Runtime does not run them and neither should a client: fetching and
/// executing on a person's behalf is the capability this product refused from the start, and an install
/// button that runs is that capability with a friendly label. The operator reads the line and decides.
///
/// Every string is validated at the declaration boundary to contain no character a shell could read as a
/// separator, so a client can present one without quoting it into something else.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderHelp {
    /// The command that signs in to this service, when it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<String>,
    /// The command that makes this service diagnose its own installation, when it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnose: Option<String>,
    /// The command that installs this service, for when no executable exists to ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

impl ProviderHelp {
    /// Whether this carries anything worth offering.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sign_in.is_none() && self.diagnose.is_none() && self.install.is_none()
    }
}

/// One provider in the fast inventory.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDescriptor {
    /// Opaque selection value.
    pub provider_id: ProviderId,
    /// Provider or manifest supplied label for presentation only.
    pub display_name: String,
    /// The editor glyph that stands for this service, for presentation only.
    ///
    /// A name, never artwork. Editors carry marks for several of these services already, and naming one is how a
    /// surface shows a conversation's service without anybody shipping a trademark. Absent when the manifest
    /// declared none, and a surface then shows whatever it shows for a service it does not recognise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Fast cached installation evidence. Listing does not start the provider.
    pub installation: InstallationObservation,
    /// This service's own commands for making itself usable, when it declares any.
    ///
    /// Absent rather than empty when nothing is declared, so a client shows nothing instead of an action
    /// that leads nowhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<ProviderHelp>,
}

/// A bounded provider inventory snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderList {
    /// Providers in registry order, including unavailable entries.
    pub providers: Vec<ProviderDescriptor>,
}

/// Install one dedicated provider inventory subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchProvidersParams {}

/// Initial provider snapshot and connection-local subscription identity.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchProvidersResult {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Exact provider snapshot at the subscription boundary.
    pub snapshot: ProviderList,
}

/// A changed complete provider inventory snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvidersChangedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// New complete provider snapshot.
    pub snapshot: ProviderList,
}

/// Why a provider inventory subscription ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderWatchEndReason {
    /// The integration grant was revoked.
    IntegrationRevoked,
    /// Scope or integration generations changed and require authenticated reconnect.
    AuthorityChanged,
    /// The Runtime provider inventory publisher stopped.
    RuntimeUnavailable,
}

/// Final typed reason for retiring a provider inventory subscription.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderWatchEndedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Structural terminal reason.
    pub reason: ProviderWatchEndReason,
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

/// What a running turn is waiting for, when it is waiting for anybody.
///
/// Structural, and deliberately only two values. A surface listing eight running sessions needs to answer one
/// question without opening any of them: which of these stopped for me? An approval identifier or the provider's
/// own wording would be conversation detail, which this protocol does not carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitingOn {
    /// A person has to answer before the turn continues.
    Person,
    /// An account limit has to lapse before the turn continues.
    Quota,
}

/// One Runtime-managed session in the immediate catalogue.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDescriptor {
    /// Stable Runtime selection value.
    pub session_id: RuntimeSessionId,
    /// Opaque provider selection value.
    pub provider_id: ProviderId,
    /// Provider-owned resume identity when the official live or stored pointer exposes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    /// Exact canonical workspace display path, already filtered through the integration's approved roots.
    pub workspace: String,
    /// Whether Runtime currently owns a provider process for this session.
    pub hot: bool,
    /// Current supervision state.
    pub lifecycle: LifecycleState,
    /// Structural stalled-turn hint derived by Core without interpreting conversation content.
    pub looks_stuck: bool,
    /// What the running turn is waiting for, when Core observed it stop for something.
    ///
    /// Optional and omitted when absent, so a client written against this revision before the field existed
    /// reads exactly what it read before. Derived from the provider's own structural turn frames, never from
    /// conversation content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<WaitingOn>,
    /// Monotonic lifecycle generation used to reject stale control actions.
    pub session_generation: u64,
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

/// Install one dedicated managed-session index subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchSessionIndexParams {}

/// Initial authorized snapshot and connection-local subscription identity.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchSessionIndexResult {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Exact authorized snapshot at the subscription boundary.
    pub snapshot: ManagedSessionList,
}

/// A changed authorized managed-session snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionIndexChangedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// New complete authorized snapshot.
    pub snapshot: ManagedSessionList,
}

/// Why a managed-session index subscription ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionIndexEndReason {
    /// The integration grant was revoked.
    IntegrationRevoked,
    /// Scope, roots, or integration generations changed and require authenticated reconnect.
    AuthorityChanged,
    /// A previously approved filesystem root no longer names the approved object.
    RootDenied,
    /// The Runtime catalogue became unavailable.
    RuntimeUnavailable,
}

/// Final typed reason for retiring a managed-session index subscription.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionIndexEndedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Structural terminal reason.
    pub reason: SessionIndexEndReason,
}

/// Select one exact Runtime-managed session.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetSessionParams {
    /// Stable opaque identity returned by the managed session catalogue.
    pub session_id: RuntimeSessionId,
}
