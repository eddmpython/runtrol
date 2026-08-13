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
    /// Discover the selected provider's current opaque model catalogue.
    #[serde(rename = "providers/listModels")]
    ProvidersListModels,
    /// Read the Runtime-managed session catalogue.
    #[serde(rename = "sessions/list")]
    SessionsList,
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
            Self::ProvidersListModels => "providers/listModels",
            Self::SessionsList => "sessions/list",
            Self::SessionsAcquireControl => "sessions/acquireControl",
            Self::SessionsRenewControl => "sessions/renewControl",
            Self::SessionsReleaseControl => "sessions/releaseControl",
            Self::SessionsSubmitInput => "sessions/submitInput",
            Self::SessionsWatchEvents => "sessions/watchEvents",
            Self::SessionsInterrupt => "sessions/interrupt",
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
            "providers/listModels" => Ok(Self::ProvidersListModels),
            "sessions/list" => Ok(Self::SessionsList),
            "sessions/acquireControl" => Ok(Self::SessionsAcquireControl),
            "sessions/renewControl" => Ok(Self::SessionsRenewControl),
            "sessions/releaseControl" => Ok(Self::SessionsReleaseControl),
            "sessions/submitInput" => Ok(Self::SessionsSubmitInput),
            "sessions/watchEvents" => Ok(Self::SessionsWatchEvents),
            "sessions/interrupt" => Ok(Self::SessionsInterrupt),
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
            RuntimeMethod::ProvidersListModels,
            RuntimeMethod::SessionsList,
            RuntimeMethod::SessionsAcquireControl,
            RuntimeMethod::SessionsRenewControl,
            RuntimeMethod::SessionsReleaseControl,
            RuntimeMethod::SessionsSubmitInput,
            RuntimeMethod::SessionsWatchEvents,
            RuntimeMethod::SessionsInterrupt,
            RuntimeMethod::SessionsEvent,
            RuntimeMethod::SessionsLagged,
            RuntimeMethod::PanicStop,
        ] {
            assert_eq!(method.as_str().parse(), Ok(method));
        }
        assert!("private/control".parse::<RuntimeMethod>().is_err());
    }
}
