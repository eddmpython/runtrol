//! THE kind table. The only place a `kind` string is written.
//!
//! A manifest names a kind and a kind selects code. This is where that selection lives, and it is a plain table of
//! values on purpose.
//!
//! # Why not a registration macro
//!
//! Measured: a distributed-slice registry returns a **silently empty slice** across a crate boundary unless the
//! binary already references the registering crate, in both debug and a fully optimized release. A driver that
//! failed to register would present as a provider that is simply absent, with nothing anywhere saying so.
//!
//! When the binary does reference the crate, a plain table is strictly better: the same cost, no macro, and a
//! missing driver is a compile error instead of a list that is quietly one shorter.
//!
//! # Why an entry may name no driver
//!
//! A build can know a kind exists and be unable to serve it: a driver behind a feature, or one nobody has written.
//! Answering "unknown kind" there would send the operator hunting for a typo they did not make. So an entry carries
//! either something that serves it or a sentence saying why this build does not.

use std::sync::Arc;

use runtrol_childproc::{Containment, Program};
use runtrol_provider::{Provider, ProviderId};

use crate::claude::ClaudeProvider;
use crate::codex::CodexProvider;

/// The manifests compiled into this binary.
///
/// Text rather than parsed values, because the loader owns parsing and there is exactly one parser. A built-in that
/// went in already parsed would be a second reading of the schema, and two readings drift.
pub const MANIFESTS: &[&str] = &[
    include_str!("../manifests/claude.toml"),
    include_str!("../manifests/codex.toml"),
];

/// Builds a driver for one kind.
///
/// A function pointer rather than a boxed closure, so the table stays a `const`. Nothing is constructed until a
/// session needs one, which is what keeps boot free of process work.
pub type MakeDriver = fn(&DriverContext) -> Box<dyn Provider>;

/// What this build can do about one kind.
#[derive(Clone, Copy)]
pub struct DriverKind {
    /// The kind, exactly as a manifest spells it.
    pub kind: &'static str,
    /// Builds the driver, when this build has one.
    pub make: Option<MakeDriver>,
    /// Why this build cannot serve it, when it cannot.
    ///
    /// A sentence an operator reads, not a code. The difference between "this build has no generic driver for that
    /// protocol" and "unknown kind" is the difference between an answer and a wild goose chase.
    pub unavailable: Option<&'static str>,
}

impl core::fmt::Debug for DriverKind {
    /// Prints the kind and whether it is served. A function pointer has nothing readable to show.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DriverKind")
            .field("kind", &self.kind)
            .field("served", &self.make.is_some())
            .field("unavailable", &self.unavailable)
            .finish()
    }
}

/// What a driver needs in order to exist.
///
/// Assembled by whoever composes the build. A driver never resolves its own program or establishes its own
/// containment: resolution is the probe's job and containment is the process's, both done once.
#[derive(Clone)]
pub struct DriverContext {
    /// Which provider this driver is being built for.
    pub provider: ProviderId,
    /// The program to run, already resolved and with its launchers unwrapped.
    pub program: Program,
    /// The containment every child joins.
    ///
    /// Shared rather than owned, because there is one containment per process and dropping it is the kill switch.
    pub contained_by: Arc<Containment>,
}

/// Every kind this build knows about.
///
/// The unserved entries are live code with a test each, not scaffolding: they are the difference between an honest
/// "not in this build" and a misleading "unknown kind".
pub const KINDS: &[DriverKind] = &[
    DriverKind {
        kind: "claude-stream-json",
        make: Some(make_claude),
        unavailable: None,
    },
    DriverKind {
        kind: "codex-app-server",
        make: Some(make_codex),
        unavailable: None,
    },
    DriverKind {
        kind: "acp",
        make: None,
        unavailable: Some("this build has no generic driver for that protocol"),
    },
    DriverKind {
        kind: "exec-oneshot",
        make: None,
        unavailable: Some("one process per turn is not a transport this build serves"),
    },
    DriverKind {
        kind: "pty",
        make: None,
        unavailable: Some("a terminal transport is not built into this binary"),
    },
];

/// Build the driver for the CLI that runs one process per session.
fn make_claude(context: &DriverContext) -> Box<dyn Provider> {
    Box::new(ClaudeProvider::new(
        context.provider,
        context.program.clone(),
        Arc::clone(&context.contained_by),
    ))
}

/// Build the driver for the CLI whose sessions share one daemon.
fn make_codex(context: &DriverContext) -> Box<dyn Provider> {
    Box::new(CodexProvider::new(
        context.provider,
        context.program.clone(),
        Arc::clone(&context.contained_by),
    ))
}

/// What this build can do about a kind.
#[must_use]
pub fn lookup(kind: &str) -> Option<&'static DriverKind> {
    KINDS.iter().find(|entry| entry.kind == kind)
}

#[cfg(test)]
mod tests {
    use runtrol_provider::Manifest;

    use super::*;

    #[test]
    fn every_built_in_manifest_reads_with_the_schema_that_ships_beside_it() {
        // A built-in that does not parse is a provider that vanishes on a fresh install, and it would vanish for
        // everybody at once. Cheap to check and the cheapest possible place to catch it.
        for text in MANIFESTS {
            let manifest: Manifest = toml::from_str(text).expect("a built-in manifest must parse");
            manifest.validate().expect("and must validate");
            assert!(
                lookup(manifest.kind.as_str()).is_some(),
                "{} names kind {:?}, which no entry declares",
                manifest.id,
                manifest.kind.as_str()
            );
        }
    }

    #[test]
    fn every_built_in_manifest_names_a_kind_this_build_serves() {
        // A built-in for a kind nothing serves would ship a provider the operator can see and cannot start. An
        // operator's own manifest may legitimately do that; one compiled in may not.
        for text in MANIFESTS {
            let manifest: Manifest = toml::from_str(text).expect("parses");
            let entry = lookup(manifest.kind.as_str()).expect("declared");
            assert!(
                entry.make.is_some(),
                "{} is built in and its kind is not served",
                manifest.id
            );
        }
    }

    #[test]
    fn a_kind_this_build_knows_and_cannot_serve_says_why() {
        // Answering "unknown kind" would send the operator looking for a typo they did not make.
        let unserved: Vec<&DriverKind> =
            KINDS.iter().filter(|entry| entry.make.is_none()).collect();
        assert!(
            !unserved.is_empty(),
            "the honest-refusal entries are what this test is about"
        );
        for entry in unserved {
            let why = entry
                .unavailable
                .unwrap_or_else(|| panic!("{entry:?} serves nothing and says nothing"));
            assert!(why.len() > 20, "{entry:?} needs a sentence, not a code");
        }
    }

    #[test]
    fn a_kind_this_build_serves_claims_no_reason_it_cannot() {
        for entry in KINDS.iter().filter(|entry| entry.make.is_some()) {
            assert_eq!(
                entry.unavailable, None,
                "{entry:?} both serves the kind and says it cannot"
            );
        }
    }

    #[test]
    fn no_kind_is_declared_twice() {
        // Two entries for one kind means two answers about what serves it, and whichever is found first wins
        // silently.
        for (index, entry) in KINDS.iter().enumerate() {
            for other in KINDS.iter().skip(index + 1) {
                assert_ne!(entry.kind, other.kind, "{} is declared twice", entry.kind);
            }
        }
    }

    #[test]
    fn a_kind_nobody_declared_is_not_found() {
        assert!(lookup("nothing-declares-this").is_none());
    }

    #[test]
    fn a_manifest_is_shipped_as_text_and_parsed_by_the_one_parser() {
        // Compiled in already parsed would be a second reading of the schema, and two readings drift. The proof is
        // that the built-in goes through exactly the same call an operator's file does.
        assert!(!MANIFESTS.is_empty());
        for text in MANIFESTS {
            assert!(
                text.contains("schema = 1"),
                "a built-in declares its format"
            );
            assert!(
                toml::from_str::<Manifest>(text).is_ok(),
                "and reads with the schema, not beside it"
            );
        }
    }
}
