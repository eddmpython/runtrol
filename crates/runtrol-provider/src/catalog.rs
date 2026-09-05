//! The model choices a provider can honestly offer at runtime.
//!
//! Model identifiers are deliberately absent from manifests and core code. A driver either asks its CLI for
//! the current account-specific catalogue, offers stable alias tokens declared by the provider manifest, or
//! says why no catalogue can be known. These values are metadata for starting a session, never conversation
//! content.

use serde::{Deserialize, Serialize};

/// The most model choices one discovery response may carry.
///
/// A provider response is untrusted input. The wire already has a byte bound, but a separate item bound keeps
/// a catalogue made of tiny entries from turning into an unbounded allocation and render loop.
pub const MAX_MODEL_CHOICES: usize = 256;

/// The most reasoning choices one model may carry.
pub const MAX_REASONING_CHOICES: usize = 32;

/// The current model information a driver can honestly provide.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum ModelCatalog {
    /// The provider enumerated its current model catalogue.
    Known {
        /// Choices in the provider's own order.
        models: Vec<ModelChoice>,
    },
    /// The provider cannot enumerate models, but declares stable alias tokens.
    Aliases {
        /// Tokens accepted where a model identifier would otherwise go.
        aliases: Vec<Box<str>>,
        /// Provider-owned reasoning choices that apply across those aliases, when discovery exposes them.
        #[serde(rename = "reasoningEfforts")]
        reasoning_efforts: Vec<ReasoningChoice>,
        /// Why these are aliases rather than an enumerated catalogue.
        why: Box<str>,
    },
    /// Stable aliases plus exact options found in provider-owned state, without claiming a complete catalogue.
    Partial {
        /// Stable tokens accepted by the CLI.
        aliases: Vec<Box<str>>,
        /// Exact provider-recorded options, in provider order.
        models: Vec<ModelChoice>,
        /// Provider-owned reasoning choices that apply to aliases and models without their own list.
        #[serde(rename = "reasoningEfforts")]
        reasoning_efforts: Vec<ReasoningChoice>,
        /// Why this is not a complete account catalogue.
        why: Box<str>,
    },
    /// The driver cannot truthfully name any choices.
    Unknown {
        /// Why discovery is unavailable.
        why: Box<str>,
    },
    /// The driver has no registered official model discovery capability.
    Unsupported {
        /// Why no official discovery capability is available.
        why: Box<str>,
    },
}

impl ModelCatalog {
    /// Whether this exact provider choice appears in the current discovery result.
    #[must_use]
    pub fn contains_model(&self, selected: &str) -> bool {
        match self {
            Self::Known { models } => models.iter().any(|model| model.id.as_ref() == selected),
            Self::Aliases { aliases, .. } => aliases.iter().any(|alias| alias.as_ref() == selected),
            Self::Partial {
                aliases, models, ..
            } => {
                aliases.iter().any(|alias| alias.as_ref() == selected)
                    || models.iter().any(|model| model.id.as_ref() == selected)
            }
            Self::Unknown { .. } | Self::Unsupported { .. } => false,
        }
    }

    /// An honest answer for a driver with no discovery binding.
    #[must_use]
    pub fn unknown(why: impl Into<Box<str>>) -> Self {
        Self::Unknown { why: why.into() }
    }

    /// An honest answer for a driver with no official discovery binding.
    #[must_use]
    pub fn unsupported(why: impl Into<Box<str>>) -> Self {
        Self::Unsupported { why: why.into() }
    }
}

/// One model the provider currently offers.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    /// The exact value to send back when starting a session.
    pub id: Box<str>,
    /// The provider's current human-readable name.
    pub display_name: Box<str>,
    /// The provider's current description.
    pub description: Box<str>,
    /// Whether the provider marks this as its default.
    pub is_default: bool,
    /// Reasoning choices supported by this model, if the provider reports them.
    pub reasoning_efforts: Vec<ReasoningChoice>,
}

/// One reasoning-effort choice reported by a provider.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningChoice {
    /// The exact value the provider accepts.
    pub id: Box<str>,
    /// The provider's current description.
    pub description: Box<str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_shape_says_whether_choices_are_known_aliases_or_unknown() {
        let answers = [
            ModelCatalog::Known { models: Vec::new() },
            ModelCatalog::Aliases {
                aliases: vec!["fast".into()],
                reasoning_efforts: Vec::new(),
                why: "the CLI exposes aliases only".into(),
            },
            ModelCatalog::Partial {
                aliases: vec!["fast".into()],
                models: Vec::new(),
                reasoning_efforts: Vec::new(),
                why: "the CLI exposes only a partial catalogue".into(),
            },
            ModelCatalog::unknown("the CLI exposes no discovery surface"),
            ModelCatalog::unsupported("the driver exposes no official discovery surface"),
        ];

        let encoded = answers
            .iter()
            .map(|answer| serde_json::to_string(answer).expect("serializable"))
            .collect::<Vec<_>>();
        for (answer, kind) in
            encoded
                .iter()
                .zip(["known", "aliases", "partial", "unknown", "unsupported"])
        {
            assert!(answer.contains(&format!(r#""kind":"{kind}""#)), "{answer}");
        }
    }
}
