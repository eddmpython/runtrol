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
    /// Read the structural provider inventory.
    #[serde(rename = "providers/list")]
    ProvidersList,
    /// Read the Runtime-managed session catalogue.
    #[serde(rename = "sessions/list")]
    SessionsList,
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
            Self::ProvidersList => "providers/list",
            Self::SessionsList => "sessions/list",
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
            "providers/list" => Ok(Self::ProvidersList),
            "sessions/list" => Ok(Self::SessionsList),
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
            RuntimeMethod::ProvidersList,
            RuntimeMethod::SessionsList,
            RuntimeMethod::PanicStop,
        ] {
            assert_eq!(method.as_str().parse(), Ok(method));
        }
        assert!("private/control".parse::<RuntimeMethod>().is_err());
    }
}
