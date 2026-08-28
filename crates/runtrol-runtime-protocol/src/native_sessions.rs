//! Provider-neutral official native session catalogue DTOs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ProviderId, RuntimeSessionId};

/// Maximum public cursor bytes after Runtime wraps provider context and authenticity.
pub const MAX_NATIVE_PUBLIC_CURSOR_BYTES: usize = 8 * 1024;

/// Maximum encoded bytes for one Runtime-authenticated native adoption proof.
pub const MAX_NATIVE_ADOPTION_TOKEN_BYTES: usize = 2 * 1024;

/// Lifetime of one boot-local native catalogue cursor.
pub const NATIVE_CURSOR_LIFETIME_MS: u64 = 5 * 60_000;

/// Select one provider, and either one approved folder or the whole machine, for native discovery.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListNativeSessionsParams {
    /// Opaque provider identity returned by `providers/list`.
    pub provider_id: ProviderId,
    /// Exact canonical root already present in the authenticated integration grant, or absent to
    /// ask for every conversation the provider will name.
    ///
    /// Absence is answered only on the owner-only local endpoint, and only by a provider whose own
    /// surface enumerates without a folder filter. A caller that omits it and gets a refusal knows
    /// to ask per folder; a caller that supplies it gets exactly the folder it named, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Runtime-wrapped opaque cursor returned by the preceding page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Ask which of one provider's conversations were written in the last few seconds.
///
/// Separate from the catalogue because it is asked often and the catalogue is not cheap: on the machine this
/// was measured, a catalogue reads every transcript's head and costs 121 ms. How a provider knows is its own
/// business (Claude Code publishes a roster of its running processes and what each one is doing); what the
/// answer means here is the same for every provider, and it is how the panel shows a turn running in a
/// conversation Runtrol did not start.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeActivityParams {
    /// Opaque provider identity returned by `providers/list`.
    pub provider_id: ProviderId,
}

/// The conversations of one provider that were written inside the activity window.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeActivity {
    /// The provider asked about.
    pub provider_id: ProviderId,
    /// Native identities written inside the window, in no particular order.
    pub active: Vec<String>,
}

/// The official provider surface used for discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogueSource {
    /// A provider-owned structured protocol method.
    OfficialProtocol,
    /// A provider-owned structured CLI command.
    OfficialCli,
    /// The provider's own store, read for the names of what it holds (identity, folder, the provider's own
    /// title and time), used when the provider publishes no listing surface. Never a conversation's content.
    ProviderStore,
}

/// Honest native catalogue coverage for the current provider and root context.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CatalogueCoverage {
    /// The official provider surface claims complete pagination for the current context.
    Complete {
        /// Provenance of the catalogue.
        source: CatalogueSource,
    },
    /// The official surface or required path filtering has a named structural limitation.
    Partial {
        /// Provenance of the catalogue.
        source: CatalogueSource,
        /// Safe stable explanation of the limitation.
        why: String,
    },
    /// No registered official enumerable provider surface exists.
    Unsupported {
        /// Safe stable explanation of the absent capability.
        why: String,
    },
}

/// Whether an officially listed session can be resumed through the same provider driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeResumeCapability {
    /// An official resume operation is available.
    Available,
    /// The provider explicitly does not advertise resume support.
    Unavailable,
    /// The official discovery surface cannot establish resume support.
    Unknown,
}

/// One root-authorized official provider-native session.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionDescriptor {
    /// Provider-owned opaque identity returned unchanged for adoption.
    pub native_session_id: String,
    /// Canonical authorized primary working directory.
    pub cwd: String,
    /// Canonical authorized additional roots in provider order.
    pub additional_directories: Vec<String>,
    /// Provider-owned presentation title, if officially supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Provider-owned official timestamp representation, if supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Officially discovered resume capability.
    pub resume: NativeResumeCapability,
    /// Existing Runtime pointer matched only by provider and native identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_managed_as: Option<RuntimeSessionId>,
    /// Short-lived Runtime proof required to adopt this exact authorized observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adoption_token: Option<String>,
}

/// One bounded provider-native session page after authorization and filtering.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSessionCatalogue {
    /// Exact provider context.
    pub provider_id: ProviderId,
    /// Honest coverage after Runtime path filtering.
    pub coverage: CatalogueCoverage,
    /// Root-authorized entries in provider order.
    pub sessions: Vec<NativeSessionDescriptor>,
    /// Runtime-wrapped opaque cursor for the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_catalogue_has_explicit_coverage_and_no_conversation_field() {
        let value = serde_json::to_value(NativeSessionCatalogue {
            provider_id: ProviderId::new("provider"),
            coverage: CatalogueCoverage::Complete {
                source: CatalogueSource::OfficialProtocol,
            },
            sessions: vec![NativeSessionDescriptor {
                native_session_id: "native".to_owned(),
                cwd: "/work".to_owned(),
                additional_directories: Vec::new(),
                title: Some("Provider title".to_owned()),
                updated_at: None,
                resume: NativeResumeCapability::Available,
                already_managed_as: None,
                adoption_token: Some("opaque-runtime-proof".to_owned()),
            }],
            next_cursor: None,
        })
        .expect("serializable");
        assert_eq!(
            value.pointer("/coverage/kind"),
            Some(&serde_json::json!("complete"))
        );
        let text = value.to_string();
        assert!(!text.contains("prompt"));
        assert!(!text.contains("reply"));
        assert!(!text.contains("preview"));
        assert!(!text.contains("transcript"));
    }
}
