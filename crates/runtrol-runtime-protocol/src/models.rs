//! Provider-neutral model discovery DTOs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProviderId;

/// Select one provider for explicit, potentially slow model discovery.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListModelsParams {
    /// Opaque provider identity returned by `providers/list`.
    pub provider_id: ProviderId,
}

/// The current model information Runtime can truthfully expose.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "coverage", rename_all = "camelCase", deny_unknown_fields)]
pub enum RuntimeModelCatalog {
    /// An official provider surface returned a bounded authoritative list.
    Known {
        /// Choices in provider order.
        models: Vec<RuntimeModelChoice>,
    },
    /// The provider exposes selectable aliases but no complete inventory.
    Aliases {
        /// Exact opaque tokens accepted by the provider.
        aliases: Vec<String>,
        /// Provider-owned reasoning choices that apply across those aliases.
        #[serde(rename = "reasoningEfforts")]
        reasoning_efforts: Vec<RuntimeReasoningChoice>,
        /// Safe structural explanation of the limitation.
        why: String,
    },
    /// An official or provider-owned surface returned a structurally limited list.
    Partial {
        /// Exact opaque alias tokens accepted by the provider.
        aliases: Vec<String>,
        /// Exact model options observed from the provider.
        models: Vec<RuntimeModelChoice>,
        /// Provider-owned reasoning choices used when a model has no narrower list.
        #[serde(rename = "reasoningEfforts")]
        reasoning_efforts: Vec<RuntimeReasoningChoice>,
        /// Safe structural explanation of the limitation.
        why: String,
    },
    /// Runtime probed but cannot currently interpret a catalogue structurally.
    Unknown {
        /// Safe structural explanation of the unknown result.
        why: String,
    },
    /// The registered driver has no official model discovery capability.
    Unsupported {
        /// Safe structural explanation of the missing capability.
        why: String,
    },
}

/// One opaque model selection reported by a provider.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeModelChoice {
    /// Exact value to return when starting a session.
    pub id: String,
    /// Provider-supplied presentation name.
    pub display_name: String,
    /// Provider-supplied structural description.
    pub description: String,
    /// Whether the provider currently marks this choice as its default.
    pub is_default: bool,
    /// Opaque reasoning choices reported for this model.
    pub reasoning_efforts: Vec<RuntimeReasoningChoice>,
}

/// One opaque reasoning-effort option reported by a provider.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeReasoningChoice {
    /// Exact value accepted by the provider.
    pub id: String,
    /// Provider-supplied structural description.
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_explicit_and_provider_choices_stay_opaque() {
        let catalogue = RuntimeModelCatalog::Known {
            models: vec![RuntimeModelChoice {
                id: "provider-owned-id".to_owned(),
                display_name: "Provider Name".to_owned(),
                description: "Provider description".to_owned(),
                is_default: true,
                reasoning_efforts: vec![RuntimeReasoningChoice {
                    id: "provider-owned-effort".to_owned(),
                    description: "Provider effort".to_owned(),
                }],
            }],
        };
        let encoded = serde_json::to_value(catalogue).expect("serializable catalogue");
        assert_eq!(
            encoded,
            serde_json::json!({
                "coverage": "known",
                "models": [{
                    "id": "provider-owned-id",
                    "displayName": "Provider Name",
                    "description": "Provider description",
                    "isDefault": true,
                    "reasoningEfforts": [{
                        "id": "provider-owned-effort",
                        "description": "Provider effort"
                    }]
                }]
            })
        );
    }

    #[test]
    fn fallback_reasoning_choices_use_the_public_camel_case_field() {
        let catalogue = RuntimeModelCatalog::Aliases {
            aliases: vec!["provider-alias".to_owned()],
            reasoning_efforts: vec![RuntimeReasoningChoice {
                id: "provider-effort".to_owned(),
                description: "Provider effort".to_owned(),
            }],
            why: "the provider exposes aliases".to_owned(),
        };
        let encoded = serde_json::to_value(catalogue).expect("serializable catalogue");
        assert_eq!(
            encoded,
            serde_json::json!({
                "coverage": "aliases",
                "aliases": ["provider-alias"],
                "reasoningEfforts": [{
                    "id": "provider-effort",
                    "description": "Provider effort"
                }],
                "why": "the provider exposes aliases"
            })
        );
    }
}
