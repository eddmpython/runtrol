//! Closed public method vocabulary.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A public Runtime method implemented by the initial read-only boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, JsonSchema, Serialize, Deserialize)]
pub enum RuntimeMethod {
    /// Negotiate a public revision and bind the Runtime instance.
    #[serde(rename = "runtime/initialize")]
    Initialize,
    /// Finish initialization before any inventory request.
    #[serde(rename = "runtime/initialized")]
    Initialized,
    /// Server-first connection-bound authentication challenge notification.
    #[serde(rename = "runtime/challenge")]
    Challenge,
    /// Create a bounded pending local enrollment.
    #[serde(rename = "integrations/requestEnrollment")]
    IntegrationsRequestEnrollment,
    /// Read the current decision for the proved pending key.
    #[serde(rename = "integrations/watchEnrollment")]
    IntegrationsWatchEnrollment,
    /// Read the authenticated integration's current grant.
    #[serde(rename = "integrations/getGrant")]
    IntegrationsGetGrant,
    /// Read the structural provider inventory.
    #[serde(rename = "providers/list")]
    ProvidersList,
    /// Watch structural provider inventory changes without polling.
    #[serde(rename = "providers/watch")]
    ProvidersWatch,
    /// Discover structural lifecycle and event capabilities for one provider.
    #[serde(rename = "providers/getCapabilities")]
    ProvidersGetCapabilities,
    /// Discover the selected provider's current opaque model catalogue.
    #[serde(rename = "providers/listModels")]
    ProvidersListModels,
    /// Discover one root-scoped official provider-native session page.
    #[serde(rename = "providers/listNativeSessions")]
    ProvidersListNativeSessions,
    /// Read the Runtime-managed session catalogue.
    #[serde(rename = "sessions/list")]
    SessionsList,
    /// Watch authorized managed-session index snapshots without polling.
    #[serde(rename = "sessions/watchIndex")]
    SessionsWatchIndex,
    /// Read one exact Runtime-managed session descriptor.
    #[serde(rename = "sessions/get")]
    SessionsGet,
    /// Start one fresh provider-native session under an approved root.
    #[serde(rename = "sessions/start")]
    SessionsStart,
    /// Adopt one officially listed native session into Runtime supervision.
    #[serde(rename = "sessions/adoptNative")]
    SessionsAdoptNative,
    /// Heat one existing Runtime-managed cold session.
    #[serde(rename = "sessions/resume")]
    SessionsResume,
    /// Acquire the one renewable write lease for a live session.
    #[serde(rename = "sessions/acquireControl")]
    SessionsAcquireControl,
    /// Renew one exact control lease generation.
    #[serde(rename = "sessions/renewControl")]
    SessionsRenewControl,
    /// Voluntarily release one exact control lease generation.
    #[serde(rename = "sessions/releaseControl")]
    SessionsReleaseControl,
    /// Submit caller-owned input without rewriting or implicit retry.
    #[serde(rename = "sessions/submitInput")]
    SessionsSubmitInput,
    /// Watch the existing bounded normalized event stream.
    #[serde(rename = "sessions/watchEvents")]
    SessionsWatchEvents,
    /// Interrupt one exact controlled live session.
    #[serde(rename = "sessions/interrupt")]
    SessionsInterrupt,
    /// Release one idle hot provider process while retaining its managed pointer.
    #[serde(rename = "sessions/cool")]
    SessionsCool,
    /// Read pending structured provider approvals for one controlled session.
    #[serde(rename = "approvals/listPending")]
    ApprovalsListPending,
    /// Answer one exact pending provider approval.
    #[serde(rename = "approvals/respond")]
    ApprovalsRespond,
    /// One changed managed-session index snapshot.
    #[serde(rename = "sessions/indexChanged")]
    SessionsIndexChanged,
    /// Final managed-session index subscription reason.
    #[serde(rename = "sessions/indexEnded")]
    SessionsIndexEnded,
    /// One changed provider inventory snapshot.
    #[serde(rename = "providers/changed")]
    ProvidersChanged,
    /// Final provider inventory subscription reason.
    #[serde(rename = "providers/watchEnded")]
    ProvidersWatchEnded,
    /// One normalized event notification.
    #[serde(rename = "sessions/event")]
    SessionsEvent,
    /// A bounded subscription was retired after lagging.
    #[serde(rename = "sessions/lagged")]
    SessionsLagged,
    /// Stop every supervised process in the safe direction.
    #[serde(rename = "runtime/panicStop")]
    PanicStop,
}

impl RuntimeMethod {
    /// The stable JSON-RPC method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "runtime/initialize",
            Self::Initialized => "runtime/initialized",
            Self::Challenge => "runtime/challenge",
            Self::IntegrationsRequestEnrollment => "integrations/requestEnrollment",
            Self::IntegrationsWatchEnrollment => "integrations/watchEnrollment",
            Self::IntegrationsGetGrant => "integrations/getGrant",
            Self::ProvidersList => "providers/list",
            Self::ProvidersWatch => "providers/watch",
            Self::ProvidersGetCapabilities => "providers/getCapabilities",
            Self::ProvidersListModels => "providers/listModels",
            Self::ProvidersListNativeSessions => "providers/listNativeSessions",
            Self::SessionsList => "sessions/list",
            Self::SessionsWatchIndex => "sessions/watchIndex",
            Self::SessionsGet => "sessions/get",
            Self::SessionsStart => "sessions/start",
            Self::SessionsAdoptNative => "sessions/adoptNative",
            Self::SessionsResume => "sessions/resume",
            Self::SessionsAcquireControl => "sessions/acquireControl",
            Self::SessionsRenewControl => "sessions/renewControl",
            Self::SessionsReleaseControl => "sessions/releaseControl",
            Self::SessionsSubmitInput => "sessions/submitInput",
            Self::SessionsWatchEvents => "sessions/watchEvents",
            Self::SessionsInterrupt => "sessions/interrupt",
            Self::SessionsCool => "sessions/cool",
            Self::ApprovalsListPending => "approvals/listPending",
            Self::ApprovalsRespond => "approvals/respond",
            Self::SessionsIndexChanged => "sessions/indexChanged",
            Self::SessionsIndexEnded => "sessions/indexEnded",
            Self::ProvidersChanged => "providers/changed",
            Self::ProvidersWatchEnded => "providers/watchEnded",
            Self::SessionsEvent => "sessions/event",
            Self::SessionsLagged => "sessions/lagged",
            Self::PanicStop => "runtime/panicStop",
        }
    }
}

impl fmt::Display for RuntimeMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RuntimeMethod {
    type Err = UnknownMethod;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "runtime/initialize" => Ok(Self::Initialize),
            "runtime/initialized" => Ok(Self::Initialized),
            "runtime/challenge" => Ok(Self::Challenge),
            "integrations/requestEnrollment" => Ok(Self::IntegrationsRequestEnrollment),
            "integrations/watchEnrollment" => Ok(Self::IntegrationsWatchEnrollment),
            "integrations/getGrant" => Ok(Self::IntegrationsGetGrant),
            "providers/list" => Ok(Self::ProvidersList),
            "providers/watch" => Ok(Self::ProvidersWatch),
            "providers/getCapabilities" => Ok(Self::ProvidersGetCapabilities),
            "providers/listModels" => Ok(Self::ProvidersListModels),
            "providers/listNativeSessions" => Ok(Self::ProvidersListNativeSessions),
            "sessions/list" => Ok(Self::SessionsList),
            "sessions/watchIndex" => Ok(Self::SessionsWatchIndex),
            "sessions/get" => Ok(Self::SessionsGet),
            "sessions/start" => Ok(Self::SessionsStart),
            "sessions/adoptNative" => Ok(Self::SessionsAdoptNative),
            "sessions/resume" => Ok(Self::SessionsResume),
            "sessions/acquireControl" => Ok(Self::SessionsAcquireControl),
            "sessions/renewControl" => Ok(Self::SessionsRenewControl),
            "sessions/releaseControl" => Ok(Self::SessionsReleaseControl),
            "sessions/submitInput" => Ok(Self::SessionsSubmitInput),
            "sessions/watchEvents" => Ok(Self::SessionsWatchEvents),
            "sessions/interrupt" => Ok(Self::SessionsInterrupt),
            "sessions/cool" => Ok(Self::SessionsCool),
            "approvals/listPending" => Ok(Self::ApprovalsListPending),
            "approvals/respond" => Ok(Self::ApprovalsRespond),
            "sessions/indexChanged" => Ok(Self::SessionsIndexChanged),
            "sessions/indexEnded" => Ok(Self::SessionsIndexEnded),
            "providers/changed" => Ok(Self::ProvidersChanged),
            "providers/watchEnded" => Ok(Self::ProvidersWatchEnded),
            "sessions/event" => Ok(Self::SessionsEvent),
            "sessions/lagged" => Ok(Self::SessionsLagged),
            "runtime/panicStop" => Ok(Self::PanicStop),
            _ => Err(UnknownMethod(value.to_owned())),
        }
    }
}

/// An incoming method name outside the public table.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown public Runtime method {0:?}")]
pub struct UnknownMethod(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_public_method_round_trips_through_its_stable_name() {
        for method in [
            RuntimeMethod::Initialize,
            RuntimeMethod::Initialized,
            RuntimeMethod::Challenge,
            RuntimeMethod::IntegrationsRequestEnrollment,
            RuntimeMethod::IntegrationsWatchEnrollment,
            RuntimeMethod::IntegrationsGetGrant,
            RuntimeMethod::ProvidersList,
            RuntimeMethod::ProvidersWatch,
            RuntimeMethod::ProvidersGetCapabilities,
            RuntimeMethod::ProvidersListModels,
            RuntimeMethod::ProvidersListNativeSessions,
            RuntimeMethod::SessionsList,
            RuntimeMethod::SessionsWatchIndex,
            RuntimeMethod::SessionsGet,
            RuntimeMethod::SessionsStart,
            RuntimeMethod::SessionsAdoptNative,
            RuntimeMethod::SessionsResume,
            RuntimeMethod::SessionsAcquireControl,
            RuntimeMethod::SessionsRenewControl,
            RuntimeMethod::SessionsReleaseControl,
            RuntimeMethod::SessionsSubmitInput,
            RuntimeMethod::SessionsWatchEvents,
            RuntimeMethod::SessionsInterrupt,
            RuntimeMethod::SessionsCool,
            RuntimeMethod::ApprovalsListPending,
            RuntimeMethod::ApprovalsRespond,
            RuntimeMethod::SessionsIndexChanged,
            RuntimeMethod::SessionsIndexEnded,
            RuntimeMethod::ProvidersChanged,
            RuntimeMethod::ProvidersWatchEnded,
            RuntimeMethod::SessionsEvent,
            RuntimeMethod::SessionsLagged,
            RuntimeMethod::PanicStop,
        ] {
            assert_eq!(method.as_str().parse(), Ok(method));
        }
        assert!("private/control".parse::<RuntimeMethod>().is_err());
    }
}
