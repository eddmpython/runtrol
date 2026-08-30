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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use runtrol_childproc::{Containment, Program};
use runtrol_provider::{AccountSpec, ModelAliases, Provider, ProviderId, StoreSpec};

use crate::acp::AcpProvider;
use crate::claude::ClaudeProvider;
use crate::codex::CodexProvider;
#[cfg(test)]
use crate::shipped::MANIFESTS;

/// Builds a driver for one kind.
///
/// A function pointer rather than a boxed closure, so the table stays a `const`. Nothing is constructed until a
/// session needs one, which is what keeps boot free of process work.
pub type MakeDriver = fn(&DriverContext) -> Box<dyn Provider>;

/// One CLI flag a driver consumes, and the honest outcome when it is absent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DriverFlag {
    /// The exact flag offered to the provider's own parser.
    pub flag: &'static str,
    /// Whether this driver cannot provide its protocol without the flag.
    pub required: bool,
    /// What becomes unavailable when the flag is absent.
    pub without_it: &'static str,
}

/// What this build can do about one kind.
#[derive(Clone, Copy)]
pub struct DriverKind {
    /// The kind, exactly as a manifest spells it.
    pub kind: &'static str,
    /// Builds the driver, when this build has one.
    pub make: Option<MakeDriver>,
    /// Flags this driver actually passes to its CLI.
    pub flags: &'static [DriverFlag],
    /// How this CLI takes part in cross-consult wiring, when it has official commands for it.
    pub consult: crate::consult::ConsultSurface,
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
            .field("flags", &self.flags)
            .field("consult", &self.consult)
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
    /// How to learn this provider's models, when its protocol cannot say.
    pub models: ModelAliases,
    /// Where this CLI keeps its conversations and its own commands over them.
    pub store: StoreSpec,
    /// How to ask this CLI where the account stands, when its manifest declares a status command.
    pub account: Option<AccountSpec>,
    /// The program to run, already resolved and with its launchers unwrapped.
    pub program: Program,
    /// Arguments the manifest declares for opening the structured transport.
    ///
    /// Kept as data all the way to the generic driver. A protocol driver that supplied its own launcher flags
    /// would turn adding a provider into a code change, which is the boundary this context exists to avoid.
    pub transport_argv: Vec<Box<str>>,
    /// Bound flags the installed CLI's own parser confirmed.
    pub available_flags: BTreeSet<Box<str>>,
    /// Optional bound flags the parser did not confirm, paired with the exact consequence the driver declared.
    pub unavailable_flags: BTreeMap<Box<str>, &'static str>,
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
        flags: crate::claude::FLAGS,
        consult: crate::claude::CONSULT,
        unavailable: None,
    },
    DriverKind {
        kind: "codex-app-server",
        make: Some(make_codex),
        flags: &[],
        consult: crate::codex::CONSULT,
        unavailable: None,
    },
    DriverKind {
        kind: "acp",
        make: Some(make_acp),
        flags: &[],
        // The generic protocol driver serves whatever CLI a manifest names, so there is no one set of
        // official wiring commands to declare for it.
        consult: crate::consult::ConsultSurface::NONE,
        unavailable: None,
    },
    DriverKind {
        kind: "exec-oneshot",
        make: None,
        flags: &[],
        consult: crate::consult::ConsultSurface::NONE,
        unavailable: Some("one process per turn is not a transport this build serves"),
    },
    DriverKind {
        kind: "pty",
        make: None,
        flags: &[],
        consult: crate::consult::ConsultSurface::NONE,
        unavailable: Some("a terminal transport is not built into this binary"),
    },
];

/// Build the driver for the CLI that runs one process per session.
fn make_claude(context: &DriverContext) -> Box<dyn Provider> {
    Box::new(ClaudeProvider::new(
        context.provider,
        context.program.clone(),
        Arc::clone(&context.contained_by),
        context.models.clone(),
        context.account.clone(),
        context.available_flags.clone(),
        context.unavailable_flags.clone(),
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

/// Build the provider-neutral ACP driver.
fn make_acp(context: &DriverContext) -> Box<dyn Provider> {
    Box::new(AcpProvider::new(
        context.provider,
        context.program.clone(),
        Arc::clone(&context.contained_by),
        context.models.clone(),
        context.store.clone(),
        context.transport_argv.clone(),
        context.account.clone(),
    ))
}

/// What this build can do about a kind.
#[must_use]
pub fn lookup(kind: &str) -> Option<&'static DriverKind> {
    KINDS.iter().find(|entry| entry.kind == kind)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

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
    fn every_shipped_provider_declares_its_terminal_store_account_and_event_surfaces() {
        // The explicit provider boundary: every service this build ships is attached through its manifest's four
        // surfaces and nothing else. A handwritten manifest missing one would be a provider the conversation
        // surface cannot open or the sidebar cannot read, shipped anyway.
        for text in MANIFESTS {
            let manifest: Manifest = toml::from_str(text).expect("shipped manifest parses");
            let tui = manifest
                .tui
                .as_ref()
                .unwrap_or_else(|| panic!("{} declares no [tui] surface", manifest.id));
            assert!(
                !tui.resume.is_empty(),
                "{} cannot reopen a conversation by identity",
                manifest.id
            );
            assert!(
                tui.env.contains_key("TERM"),
                "{} names no terminal type for its TUI",
                manifest.id
            );
            assert!(
                !manifest.store.location.is_empty() && manifest.store.format.is_some(),
                "{} declares no [store]",
                manifest.id
            );
            assert!(
                manifest.account.is_some(),
                "{} declares no [account]",
                manifest.id
            );
            assert!(
                manifest.events.is_some(),
                "{} declares no [events]",
                manifest.id
            );
        }
    }

    #[test]
    fn every_built_in_has_a_distinct_name_in_the_sidebar() {
        let mut names = BTreeSet::new();
        for text in MANIFESTS {
            let manifest: Manifest = toml::from_str(text).expect("built-in manifest parses");
            assert!(
                names.insert(manifest.display_name.to_string()),
                "{} repeats the sidebar name {:?}",
                manifest.id,
                manifest.display_name
            );
        }
    }

    #[test]
    fn an_acp_manifest_is_discovered_from_the_local_path() {
        const CHILD_MARKER: &str = "RUNTROL_ACP_DISCOVERY_FIXTURE";
        const TEST_NAME: &str = "kinds::tests::an_acp_manifest_is_discovered_from_the_local_path";

        // A manifest names bare executables and nothing else, so what this proves is that an ACP service is
        // found on the operator's own search path. Runtime resolves what is already installed; it has never had
        // a way to fetch one, and a manifest that named a downloader would be that way.
        let manifest: Manifest = toml::from_str(
            r#"
schema = 1
id = "fixture-acp"
display_name = "ACP Fixture"
kind = "acp"

[bin]
names = ["runtrol-acp-discovery-fixture"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
"#,
        )
        .expect("the external ACP fixture manifest parses");
        let bare = manifest
            .bin
            .names
            .first()
            .expect("the ACP fixture manifest names its executable")
            .as_ref()
            .to_owned();
        if let Some(expected) = std::env::var_os(CHILD_MARKER) {
            let resolved = runtrol_core::locate(&manifest).expect("the generated adapter resolves");
            assert_eq!(
                std::fs::canonicalize(resolved.path().as_std_path()).expect("resolved path exists"),
                std::fs::canonicalize(expected).expect("fixture path exists"),
                "the external manifest found the locally installed executable"
            );
            return;
        }

        let test_binary = std::env::current_exe().expect("current test binary");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "runtrol-acp-discovery-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("fixture directory");
        let executable = directory.join(if cfg!(windows) {
            format!("{bare}.exe")
        } else {
            bare.clone()
        });
        std::fs::copy(&test_binary, &executable).expect("fixture executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&executable)
                .expect("fixture metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).expect("fixture is executable");
        }
        let mut search_paths = vec![directory.clone()];
        if let Some(current) = std::env::var_os("PATH") {
            search_paths.extend(std::env::split_paths(&current));
        }
        let search_path = std::env::join_paths(search_paths).expect("fixture search path");
        let status = std::process::Command::new(&test_binary)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env("PATH", search_path)
            .env(CHILD_MARKER, &executable)
            .status()
            .expect("child test starts");
        std::fs::remove_dir_all(&directory).expect("fixture cleanup");
        assert!(status.success(), "ACP discovery child failed");
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
