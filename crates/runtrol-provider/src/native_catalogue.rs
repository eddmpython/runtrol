//! Official provider-native session discovery without transcript or storage inspection.
//!
//! A driver may expose this surface only when the provider has an official enumerable protocol or CLI command.
//! Paths and presentation metadata remain provider-owned observations. Runtime must canonicalize and authorize every
//! path before returning an entry to a consumer.

use serde::{Deserialize, Serialize};

use crate::{AbsPath, NativeSessionId};

/// The most entries a provider may return in one page.
pub const MAX_NATIVE_SESSION_ITEMS: usize = 100;

/// The most additional workspace roots one provider entry may carry.
pub const MAX_NATIVE_ADDITIONAL_DIRECTORIES: usize = 32;

/// The maximum byte length of an opaque provider pagination cursor.
pub const MAX_NATIVE_CURSOR_BYTES: usize = 4 * 1024;

/// The maximum byte length of provider-owned presentation text.
pub const MAX_NATIVE_TITLE_BYTES: usize = 4 * 1024;

/// The maximum byte length of a provider-owned timestamp string.
pub const MAX_NATIVE_TIMESTAMP_BYTES: usize = 128;

/// One explicit provider catalogue request, over one folder or over the whole machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionQuery {
    /// Canonical folder used as the provider's official working-directory filter, or `None` for
    /// every conversation the provider will name.
    ///
    /// Measured 2026-08-20 against the installed CLIs: four of the five answer without a folder
    /// and every returned row carries its own `cwd` (codex `thread/list` omits its optional `cwd`
    /// filter, ACP `session/list` treats `cwd` as a filter that means "all" when absent, and
    /// `cline history` has no folder argument at all). The narrowing was ours, not theirs, and it
    /// cost the product its one promise: every conversation on the machine in one list. A driver
    /// that genuinely cannot answer without a folder says so through
    /// [`ProviderCapabilities::native_session_catalogue`] and is asked per folder instead.
    pub root: Option<AbsPath>,
    /// Opaque provider cursor from the immediately preceding page.
    pub cursor: Option<Box<str>>,
    /// Maximum entries Runtime is willing to receive.
    pub limit: u16,
}

impl NativeSessionQuery {
    /// The folder this query filters on, for a driver that can only ask about one.
    ///
    /// A driver reaches for this only after declaring it cannot enumerate the machine; the daemon
    /// never sends it a folderless query, so the absence is a contract violation rather than a
    /// case to paper over.
    #[must_use]
    pub fn required_root(&self) -> Option<&AbsPath> {
        self.root.as_ref()
    }
}

/// The official surface that produced a provider catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeCatalogueSource {
    /// A provider-owned structured protocol method.
    OfficialProtocol,
    /// A provider-owned structured CLI command.
    OfficialCli,
}

/// Honest coverage for one provider catalogue page.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NativeCatalogueCoverage {
    /// The official surface claims its pagination covers every matching session in the current provider context.
    Complete {
        /// Provenance of the official result.
        source: NativeCatalogueSource,
    },
    /// The official surface or safe Runtime filtering has a named structural limitation.
    Partial {
        /// Provenance of the official result.
        source: NativeCatalogueSource,
        /// Stable structural explanation of the limitation.
        why: Box<str>,
    },
    /// The driver has no registered official enumerable surface.
    Unsupported {
        /// Stable structural explanation of the absent capability.
        why: Box<str>,
    },
}

impl NativeCatalogueCoverage {
    /// An honest default for a driver with no official enumerable surface.
    #[must_use]
    pub fn unsupported(why: impl Into<Box<str>>) -> Self {
        Self::Unsupported { why: why.into() }
    }
}

/// Whether the same official provider surface can resume a listed session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeResumeCapability {
    /// The provider advertises an official resume operation.
    Available,
    /// The provider explicitly does not advertise a resume operation.
    Unavailable,
    /// The discovery surface cannot establish resume support.
    Unknown,
}

/// One provider-owned native session observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionEntry {
    /// Provider-owned opaque session identity.
    pub native: NativeSessionId,
    /// Provider-reported primary working directory, not yet trusted as authority.
    pub cwd: Box<str>,
    /// Provider-reported additional roots, not yet trusted as authority.
    pub additional_directories: Vec<Box<str>>,
    /// Provider-owned presentation title, never generated or interpreted by Runtime.
    pub title: Option<Box<str>>,
    /// Provider-owned official timestamp representation.
    pub updated_at: Option<Box<str>>,
    /// Officially discovered resume support.
    pub resume: NativeResumeCapability,
}

/// One bounded official provider-native session page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionCatalogue {
    /// Honest coverage and provenance.
    pub coverage: NativeCatalogueCoverage,
    /// Entries in provider order.
    pub sessions: Vec<NativeSessionEntry>,
    /// Opaque provider cursor for the next page.
    pub next_cursor: Option<Box<str>>,
}

impl NativeSessionCatalogue {
    /// An honest result for a driver without an official enumerable surface.
    #[must_use]
    pub fn unsupported(why: impl Into<Box<str>>) -> Self {
        Self {
            coverage: NativeCatalogueCoverage::unsupported(why),
            sessions: Vec::new(),
            next_cursor: None,
        }
    }
}

/// One provider-native conversation to delete, through the provider's own surface.
///
/// Deleting is the provider's act, never runtrol's: runtrol holds no copy and removes nothing itself, it asks
/// the CLI that owns the conversation to remove it (codex `thread/delete`, cline `history delete`). A provider
/// with no such surface says so and the conversation stays where it is. The folder travels with the request
/// because a CLI that scopes its store by folder is asked in that folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionDeletion {
    /// The provider's own name for the conversation.
    pub native: NativeSessionId,
    /// Where the conversation ran.
    pub cwd: AbsPath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_an_explicit_empty_product_state() {
        let catalogue = NativeSessionCatalogue::unsupported("no official enumerable surface");
        assert!(matches!(
            catalogue.coverage,
            NativeCatalogueCoverage::Unsupported { ref why }
                if why.as_ref() == "no official enumerable surface"
        ));
        assert!(catalogue.sessions.is_empty());
        assert!(catalogue.next_cursor.is_none());
    }
}
