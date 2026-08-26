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
    /// The mode tokens this service accepts a runtrol switch to, when its vocabulary is a manifest fact.
    ///
    /// Empty for a protocol that announces its modes per session; the session's own announcement is then
    /// the list a surface offers. The daemon enforces the same boundary on `sessions/setMode`, so this list
    /// is presentation, never authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switchable_modes: Vec<String>,
    /// Where the operator's account with this service stands, by the service's own status surface.
    ///
    /// Absent until Runtime has asked the service once; a surface then says "not checked yet" rather than
    /// inventing a green light. Present for every installed service afterwards, including one whose
    /// service publishes no way to ask, which the report says in so many words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<ProviderAccount>,
}

/// Whether the operator is signed in to one service, by that service's own word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderAccountStatus {
    /// The service said the operator is signed in.
    SignedIn,
    /// The service said nobody is signed in.
    SignedOut,
    /// The service publishes no way to ask.
    Unpublished,
}

/// One service's account report, structured fields only.
///
/// Limit windows a service reports on request (outside any turn) do not travel here: they land in the
/// usage list beside the windows a turn reports, so a surface reads one gauge per service.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAccount {
    /// Signed in, signed out, or nothing to ask.
    pub status: ProviderAccountStatus,
    /// The plan token exactly as the service wrote it, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// How the operator is signed in, as the service names it, when it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Why nothing can be asked, in the service's own terms, for an unpublished status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Why this signed-in account has no limit windows, when the service has a limits surface that did not
    /// answer.
    ///
    /// The one absence that is runtrol's own problem rather than the service's. Without it a surface has to
    /// choose between saying the service publishes nothing (untrue) and saying a number is coming (also
    /// untrue), and both send the reader somewhere useless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits_unread: Option<String>,
    /// When the service answered, in unix milliseconds, which is how a surface says how stale it is.
    pub checked_at_ms: u64,
}

/// A bounded provider inventory snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderList {
    /// Providers in registry order, including unavailable entries.
    pub providers: Vec<ProviderDescriptor>,
}

/// Where each account stands against its limits, by each provider's own latest report.
///
/// Structured fields only, never the provider's verbatim payload: that payload rides the session event stream
/// under session-output authority, and this list answers under provider authority. A gauge absent from the list
/// means that provider has not reported since the Runtime started, which is different from a limit not existing,
/// and a surface says "no report yet" rather than inventing a green light.
#[derive(Clone, Debug, Default, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUsageList {
    /// Latest reports in provider order, one per provider that has reported.
    pub providers: Vec<ProviderUsageGauge>,
}

/// One provider's most recent limit report.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUsageGauge {
    /// Opaque selection value.
    pub provider_id: ProviderId,
    /// A limit is blocking right now, by the provider's own word.
    pub reached: bool,
    /// Every limit window the provider described, shortest first.
    ///
    /// A list, because a plan is not two windows: measured, one service publishes a five-hour window, a
    /// whole-account week and a week scoped to one model, and another publishes a short and a long window for
    /// each metered model. A surface draws them all rather than being handed the two a driver chose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<ProviderUsageWindow>,
    /// The latest running spend the provider stated, when it states one.
    ///
    /// Absent for a provider that reports only limits. The newest report wins, so this is the most recent
    /// turn's cost as the provider gave it, never a total runtrol summed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<ProviderUsageCost>,
    /// Tokens the account spent today by the provider's own daily count, when it publishes one.
    ///
    /// Read on request from the provider's own usage surface (the manifest's account protocol), never summed by
    /// Runtime. Absent for a provider that publishes limits only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_today: Option<u64>,
    /// When the report arrived, in unix milliseconds, which is how a surface says how stale it is.
    pub at_ms: u64,
}

/// A changed account usage snapshot, delivered on the provider inventory subscription.
///
/// Usage moves with every turn and every probe; a subscriber draws it the moment it changes instead of
/// asking `providers/usage` on a clock. Sent once right after the subscription is installed, so a
/// subscriber never needs the request at all.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvidersUsageChangedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// New complete usage snapshot.
    pub snapshot: ProviderUsageList,
}

/// Money a provider reported spending, exactly as it stated it.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUsageCost {
    /// How much.
    pub amount: f64,
    /// The currency as the provider wrote it, never converted.
    pub currency: String,
}

/// One rate limit window, as far as the provider described it.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUsageWindow {
    /// Stable identity within one provider's report, as that provider names the window.
    ///
    /// A surface keys its row on this, so a window keeps its place when a later report adds a sibling.
    pub id: String,
    /// What the provider calls this window for a person to read, when it names one.
    ///
    /// The provider's own label and nothing composed here. Absent when it named none, and a surface then
    /// says the window's length instead of inventing a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What this limit is scoped to, when the provider scopes it (one model, one surface).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The provider says this window is the one governing right now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub governing: bool,
    /// How much of the window is used, as a percentage, when the provider reports one.
    ///
    /// Optional because it is not universal: measured, one provider reports which window governs and when it
    /// resets while saying nothing about how full it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u8>,
    /// When it resets, in unix milliseconds, when the provider says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    /// How long the window is, in minutes, when the provider says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
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
