//! Which providers exist, built once at boot.
//!
//! A registry answers one question: given a provider id, what did somebody declare about it and can this
//! build serve it. It is built once from the manifests found on this machine and then only read, because a
//! provider set that changed under a running session would mean a session whose driver stopped existing.
//!
//! # What a registry is not
//!
//! It does not spawn anything, resolve a binary, or probe a version. It holds declarations. Keeping it to
//! that is what makes it cheap enough to build on every start and testable without a filesystem or a child
//! process.
//!
//! # No provider is named here
//!
//! Not in this module, not anywhere in this crate. A manifest names a kind and a kind selects code from a
//! table this crate is handed. See [`kind`] for why that indirection is the mechanism rather than the
//! decoration.

pub mod kind;
pub mod load;

use std::io;

use runtrol_provider::{AbsPath, Manifest, ManifestError, PathError, ProviderId};

pub use kind::{KindEntry, KindStatus, KindTable};
pub use load::{Loaded, Origin, Rejected, Scan, Shadowed};

/// A manifest was found and could not be used.
///
/// One variant per fix the operator would make: the file's shape, what it says, or whether it can be read at
/// all. Collapsing them would mean an operator reading "something is wrong with your manifest".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The file is not readable as its format.
    ///
    /// The message comes from the parser and carries the line, which is the part somebody editing a file
    /// needs. Wrapping it in words of our own would lose that.
    #[error("{detail}")]
    Syntax {
        /// What the parser said, including where.
        detail: String,
    },

    /// The file parsed and does not describe something usable.
    #[error(transparent)]
    Invalid {
        /// What the schema refused.
        #[from]
        source: ManifestError,
    },

    /// The file or directory could not be read.
    #[error("cannot read: {detail}")]
    Read {
        /// What class of failure the OS reported.
        kind: io::ErrorKind,
        /// What the OS said, verbatim.
        detail: String,
    },

    /// A file name in the directory is not one runtrol can form a path from.
    ///
    /// Reported rather than skipped. A file that is silently not read is a file the operator believes is in
    /// effect.
    #[error("cannot use the file name {name:?}: {source}")]
    Name {
        /// The name as the filesystem holds it.
        name: String,
        /// Why a path could not be formed.
        source: PathError,
    },

    /// An on-disk manifest tried to claim update authority.
    ///
    /// Update declarations select executable product code. Allowing a shadow manifest to carry one would turn
    /// provider discovery into a command-execution extension point.
    #[error("update declarations are accepted only from manifests compiled into runtrol")]
    UpdateAuthority,
}

/// One provider, as declared.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Provider {
    /// What it declares about itself.
    pub manifest: Manifest,
    /// Where that declaration was read from.
    pub origin: Origin,
    /// What this build can do about its kind.
    ///
    /// Decided at boot rather than at start time, so that "this build cannot serve that" is something the
    /// operator sees in a list instead of discovering when they press the button.
    pub kind: KindStatus,
}

impl Provider {
    /// The id this provider is known by.
    #[must_use]
    pub const fn id(&self) -> ProviderId {
        self.manifest.id
    }

    /// Whether a session could be started for it.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        matches!(self.kind, KindStatus::Available)
    }
}

/// Every provider this machine declares, and everything that went wrong finding them.
///
/// Built once. Nothing mutates it afterwards, which is why a running session can hold a provider id and know
/// it still means what it meant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProviderRegistry {
    /// The providers, one per id, in the order they were read.
    providers: Vec<Provider>,
    /// Manifests that were replaced by a later one with the same id.
    shadowed: Vec<Shadowed>,
    /// Files that were found and refused.
    rejected: Vec<Rejected>,
}

impl ProviderRegistry {
    /// Build a registry from the three sources, in order.
    ///
    /// `built_in` is the manifest text compiled into this binary, `beside_executable` and `operator` are the
    /// two directories, either of which may be absent. Nothing here fails: a source that cannot be read
    /// becomes a rejection, and the providers that could be read still work.
    #[must_use]
    pub fn build(
        built_in: &[&str],
        beside_executable: Option<&AbsPath>,
        operator: Option<&AbsPath>,
        kinds: &KindTable,
    ) -> Self {
        let mut scan = Scan::default();
        for text in built_in {
            scan.take_built_in(text);
        }
        if let Some(directory) = beside_executable {
            scan.take_directory(directory);
        }
        // Last, so that whatever the operator wrote wins.
        if let Some(directory) = operator {
            scan.take_directory(directory);
        }

        let (loaded, shadowed, rejected) = scan.resolve();
        let providers = loaded
            .into_iter()
            .map(|one| Provider {
                kind: kinds.lookup(&one.manifest.kind),
                manifest: one.manifest,
                origin: one.origin,
            })
            .collect();

        Self {
            providers,
            shadowed,
            rejected,
        }
    }

    /// One provider by id.
    #[must_use]
    pub fn get(&self, id: ProviderId) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.id() == id)
    }

    /// Every provider, in the order they were read.
    ///
    /// Includes the ones this build cannot serve. An operator with a manifest for a kind this binary has no
    /// driver for should see it in the list, marked, rather than wonder where it went.
    pub fn all(&self) -> impl Iterator<Item = &Provider> {
        self.providers.iter()
    }

    /// Every provider a session could be started for.
    pub fn usable(&self) -> impl Iterator<Item = &Provider> {
        self.providers
            .iter()
            .filter(|provider| provider.is_usable())
    }

    /// How many providers there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether no provider was declared at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Manifests that were replaced by a later one with the same id.
    ///
    /// Something to put in front of a person once, at startup. An operator who shadowed a built-in on
    /// purpose wants to see it confirmed, and one who did it by reusing an id needs to know.
    #[must_use]
    pub fn shadowed(&self) -> &[Shadowed] {
        &self.shadowed
    }

    /// Files that were found and refused, with the path and the reason.
    #[must_use]
    pub fn rejected(&self) -> &[Rejected] {
        &self.rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest for a given id and kind, as text.
    fn text(id: &str, kind: &str) -> String {
        format!(
            "schema = 1\nid = \"{id}\"\ndisplay_name = \"{id}\"\nkind = \"{kind}\"\n[bin]\nnames = [\"{id}\"]\n"
        )
    }

    /// A table with one served kind and one known and not served.
    const TABLE: &[KindEntry] = &[
        KindEntry {
            kind: "example-structured",
            unavailable: None,
        },
        KindEntry {
            kind: "example-heavy",
            unavailable: Some("this build does not include the heavy transport"),
        },
    ];

    fn registry(manifests: &[String]) -> ProviderRegistry {
        let texts: Vec<&str> = manifests.iter().map(String::as_str).collect();
        ProviderRegistry::build(&texts, None, None, &KindTable::new(TABLE))
    }

    #[test]
    fn a_registry_built_from_built_ins_alone_works() {
        // A fresh install has no files anywhere and still has providers.
        let registry = registry(&[
            text("first", "example-structured"),
            text("second", "example-structured"),
        ]);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.usable().count(), 2);
        assert!(registry.rejected().is_empty());
        assert!(registry.shadowed().is_empty());
    }

    #[test]
    fn a_provider_is_found_by_the_id_it_declared() {
        let registry = registry(&[text("codex", "example-structured")]);
        let id = ProviderId::parse("codex").expect("valid");
        let provider = registry.get(id).expect("declared");
        assert_eq!(provider.id(), id);
        assert_eq!(provider.origin, Origin::BuiltIn);
        assert!(provider.is_usable());

        let absent = ProviderId::parse("nothing").expect("valid");
        assert!(registry.get(absent).is_none());
    }

    #[test]
    fn a_provider_this_build_cannot_serve_is_listed_and_marked_rather_than_hidden() {
        // Hiding it would mean an operator with a perfectly good manifest wondering where their provider
        // went. Listing it with the reason is the answer.
        let registry = registry(&[
            text("served", "example-structured"),
            text("heavy", "example-heavy"),
        ]);

        assert_eq!(registry.len(), 2, "both are listed");
        assert_eq!(
            registry.all().count(),
            2,
            "and both come out of the listing"
        );
        assert_eq!(registry.usable().count(), 1, "one can be started");
        assert!(
            registry.all().any(|provider| !provider.is_usable()),
            "the unusable one has to be visible, or the operator cannot see why"
        );

        let heavy = registry
            .get(ProviderId::parse("heavy").expect("valid"))
            .expect("listed");
        match heavy.kind {
            KindStatus::Unavailable { why } => assert!(why.contains("this build"), "{why}"),
            ref other => panic!("expected a named unavailability, got {other:?}"),
        }
    }

    #[test]
    fn a_kind_no_table_entry_names_is_unknown_and_not_unavailable() {
        // Two different situations with two different messages: "this build cannot" versus "nothing
        // declares that at all", which is the only case where checking the spelling is honest advice.
        let registry = registry(&[text("mystery", "nothing-declares-this")]);
        let provider = registry
            .get(ProviderId::parse("mystery").expect("valid"))
            .expect("listed");
        assert_eq!(provider.kind, KindStatus::Unknown);
        assert!(!provider.is_usable());
    }

    #[test]
    fn the_availability_of_a_kind_is_decided_at_boot_and_not_at_the_button() {
        // The operator finds out from a list rather than from a failed start. That is the whole reason the
        // status is stored on the provider instead of being asked for later.
        let registry = registry(&[text("heavy", "example-heavy")]);
        let provider = registry
            .get(ProviderId::parse("heavy").expect("valid"))
            .expect("listed");
        assert!(matches!(provider.kind, KindStatus::Unavailable { .. }));
    }

    #[test]
    fn a_build_with_no_drivers_serves_nothing_and_says_so() {
        let declared = text("anything", "example-structured");
        let empty = ProviderRegistry::build(&[&declared], None, None, &KindTable::empty());
        assert_eq!(empty.len(), 1, "the declaration is still there");
        assert_eq!(empty.usable().count(), 0, "and nothing can serve it");
    }

    #[test]
    fn a_registry_with_nothing_at_all_is_empty_rather_than_a_failure() {
        let nothing = ProviderRegistry::build(&[], None, None, &KindTable::empty());
        assert!(nothing.is_empty());
        assert_eq!(nothing.all().count(), 0);
    }

    #[test]
    fn a_broken_manifest_does_not_stop_the_others_from_registering() {
        let registry = registry(&[
            text("good", "example-structured"),
            "this is not toml at all".to_owned(),
        ]);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.rejected().len(), 1);
        assert!(matches!(
            registry.rejected().first().map(|one| &one.why),
            Some(RegistryError::Syntax { .. })
        ));
    }

    #[test]
    fn a_later_manifest_shadows_an_earlier_one_and_the_shadowing_is_reported() {
        let mut first = text("codex", "example-structured");
        first = first.replace("display_name = \"codex\"", "display_name = \"Theirs\"");
        let mut second = text("codex", "example-structured");
        second = second.replace("display_name = \"codex\"", "display_name = \"Mine\"");

        let registry = registry(&[first, second]);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry
                .get(ProviderId::parse("codex").expect("valid"))
                .map(|one| &*one.manifest.display_name),
            Some("Mine")
        );
        assert_eq!(registry.shadowed().len(), 1);
    }

    #[test]
    fn every_reason_a_manifest_is_refused_reads_as_its_own_sentence() {
        // Three different mistakes with three different fixes. An operator reading one message has to know
        // which of the three they made.
        let syntax = RegistryError::Syntax {
            detail: "expected `=` at line 3".to_owned(),
        };
        let invalid = RegistryError::Invalid {
            source: ManifestError::Empty { field: "bin.names" },
        };
        let unreadable = RegistryError::Read {
            kind: io::ErrorKind::PermissionDenied,
            detail: "access is denied".to_owned(),
        };

        assert!(syntax.to_string().contains("line 3"));
        assert!(invalid.to_string().contains("bin.names"));
        assert!(unreadable.to_string().contains("access is denied"));
        assert_ne!(syntax.to_string(), invalid.to_string());
    }
}
