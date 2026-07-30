//! Assembling the pieces into something that can run.
//!
//! The one place that knows about all of them. Every other crate here is deliberately unable to see most of the
//! others: the kernel cannot see a driver, a driver cannot see storage, the command surface cannot see either. Those
//! missing edges are what make the architecture checkable, and the price of having them is that somebody has to do the
//! joining. This is that somebody.
//!
//! # The order is not arbitrary
//!
//! Containment is first, before anything could have started a child. Establishing it later would leave whatever was
//! already running outside it on some platforms, which is the kind of partial guarantee that reads as a full one.
//!
//! Then the home, because everything else lives inside it. Then the providers, which is reading files and no more:
//! nothing is probed and no process is started, so a start costs no more than reading a directory.
//!
//! # What composing does not do
//!
//! It does not probe. Measured, a cold start of one of these CLIs costs 300 to 900 ms before it prints anything, so
//! probing every provider here would put a second of nothing in front of the operator's first list. The probe happens
//! when something needs the answer, and its answer is remembered against the binary's own identity.

use std::sync::Arc;

use runtrol_childproc::Containment;
use runtrol_core::registry::{KindEntry, KindTable, ProviderRegistry};
use runtrol_core::{HomeError, RuntrolHome};
use runtrol_drivers::{Builtin, DriverKind};

/// The daemon could not be assembled.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ComposeError {
    /// Children could not be made to die with this process.
    ///
    /// Not worked around and not downgraded. Starting agents that cannot be contained is the outcome the containment
    /// design exists to prevent, so the daemon refuses to start rather than running with the guarantee quietly absent.
    #[error("cannot contain the agents this daemon would start: {0}")]
    Containment(#[from] runtrol_childproc::SpawnError),

    /// runtrol's own directory could not be established.
    #[error(transparent)]
    Home(#[from] HomeError),
}

/// Everything a running daemon holds.
pub struct Composed {
    /// runtrol's own directory, and every path inside it.
    pub home: RuntrolHome,
    /// The guarantee that children die with this process.
    ///
    /// Shared, because every driver hands it to every child it starts, and held for the process lifetime because
    /// dropping it is the kill.
    pub containment: Arc<Containment>,
    /// Which providers exist, and what this build can do about each.
    pub registry: ProviderRegistry,
    /// Who has been granted what.
    ///
    /// Empty at every start, which is what "default deny" means when nothing has paired yet: a device this daemon
    /// has never heard of holds nothing, and the only call that adds authority takes a witness that somebody was
    /// at the machine. Persisting grants across restarts arrives with the pairing surface that creates them; until
    /// then, an empty ledger is the honest state rather than a placeholder, because there is nothing to load.
    pub granted: runtrol_security::GrantLedger,
    /// The table that turns a kind into a driver.
    ///
    /// Kept because building a driver is deferred: it needs a resolved program, which needs a probe, which happens when
    /// something asks rather than at boot.
    pub kinds: &'static [DriverKind],
}

impl Composed {
    /// Assemble a daemon.
    ///
    /// `home` is the operator's own choice when they made one, and the platform's directory otherwise.
    ///
    /// # Errors
    ///
    /// [`ComposeError::Containment`] when children cannot be made to die with this process, [`ComposeError::Home`] when
    /// runtrol's directory cannot be established. Both stop the start: a daemon that cannot contain its agents or
    /// cannot find its own files is worse than no daemon.
    pub fn assemble(home: Option<&str>, builtin: Builtin) -> Result<Self, ComposeError> {
        // First, before any child could exist.
        let containment = Arc::new(Containment::establish()?);

        let home = match home {
            Some(chosen) => RuntrolHome::open_at(chosen)?,
            None => RuntrolHome::open()?,
        };

        let registry = load(&home, builtin);
        Ok(Self {
            home,
            containment,
            registry,
            granted: runtrol_security::GrantLedger::new(),
            kinds: builtin.kinds,
        })
    }

    /// Assemble everything except the containment.
    ///
    /// The containment cannot be established in a test: on one platform it puts the calling process into the group it
    /// is about to kill, which terminates the runner. Measured, and the reason the guarantee is proven by an integration
    /// test with a process it is allowed to kill.
    ///
    /// So this exists, and what it hands back is honest about what it is: a containment that holds nothing, which
    /// reports the weaker promise and refuses to claim a kill it did not perform. Everything else composing does is the
    /// same code.
    ///
    /// # Errors
    ///
    /// [`ComposeError::Home`] when runtrol's directory cannot be established.
    #[cfg(test)]
    pub(crate) fn for_tests(home: &str, builtin: Builtin) -> Result<Self, ComposeError> {
        let home = RuntrolHome::open_at(home)?;
        let registry = load(&home, builtin);
        Ok(Self {
            home,
            containment: Arc::new(Containment::without_any()),
            registry,
            granted: runtrol_security::GrantLedger::new(),
            kinds: builtin.kinds,
        })
    }

    /// What this build can do about a kind, by the name a manifest spells.
    #[must_use]
    pub fn driver_for(&self, kind: &str) -> Option<&'static DriverKind> {
        self.kinds.iter().find(|entry| entry.kind == kind)
    }
}

/// Read the providers this machine declares.
///
/// Files only. The order is fixed so that whatever the operator wrote wins, and it is the loader's order rather than
/// this function's: all that happens here is naming the operator's directory as the last source.
fn load(home: &RuntrolHome, builtin: Builtin) -> ProviderRegistry {
    let kinds = KindTable::new(
        builtin
            .kinds
            .iter()
            .map(|entry| KindEntry {
                kind: entry.kind,
                // The kernel's table is data and this one carries a constructor, so the conversion is the constructor
                // being dropped. That asymmetry is the seam: the kernel decides whether a kind is served and never how.
                unavailable: entry.unavailable,
            })
            .collect::<Vec<_>>(),
    );

    ProviderRegistry::build(
        builtin.manifests,
        // The directory beside the executable is how a packaged build ships an extra provider. Absent here until there
        // is a packaged build to ship one, and naming it before then would be a path with no writer.
        None,
        Some(home.paths().providers()),
        &kinds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when the test ends.
    struct Scratch {
        root: String,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("runtrol-compose-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            Self {
                root: root
                    .to_str()
                    .expect("the temporary path is UTF-8")
                    .to_owned(),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    /// The registry as composing builds it, without establishing containment.
    ///
    /// Containment cannot be established in a test: on one platform it puts the calling process into the group it is
    /// about to kill, which terminates the runner. Measured, and the reason the guarantee is proven by an integration
    /// test with a process it is allowed to kill. Everything else composing does is exercised here.
    fn registry_of(scratch: &Scratch) -> (RuntrolHome, ProviderRegistry) {
        let home = RuntrolHome::open_at(&scratch.root).expect("a fresh home opens");
        let registry = load(&home, runtrol_drivers::builtin());
        (home, registry)
    }

    #[test]
    fn a_fresh_machine_has_providers_with_no_file_anywhere() {
        // The manifests are compiled in, so a first run is not an empty list with instructions attached.
        let scratch = Scratch::make("fresh");
        let (_home, registry) = registry_of(&scratch);

        assert!(!registry.is_empty(), "a fresh install has providers");
        assert!(
            registry.rejected().is_empty(),
            "and none of the built-ins is refused: {:?}",
            registry.rejected()
        );
        assert_eq!(
            registry.usable().count(),
            registry.len(),
            "every built-in names a kind this build serves"
        );
    }

    #[test]
    fn the_operators_own_file_replaces_a_built_in() {
        // The whole reason the discovery order is fixed. An operator whose CLI moved must be able to fix it without
        // editing runtrol, and the proof is that a file with a built-in's id wins.
        let scratch = Scratch::make("shadow");
        let (home, before) = registry_of(&scratch);
        let existing = before
            .all()
            .next()
            .expect("at least one built-in")
            .manifest
            .clone();

        let path = home
            .paths()
            .providers()
            .join(&format!("{}.toml", existing.id))
            .expect("a valid file name");
        let mine = format!(
            "schema = 1\nid = \"{}\"\ndisplay_name = \"Mine\"\nkind = \"{}\"\n[bin]\nnames = [\"mine\"]\n",
            existing.id,
            existing.kind.as_str()
        );
        std::fs::write(path.as_std_path(), mine).expect("write the operator's file");

        let after = load(&home, runtrol_drivers::builtin());
        assert_eq!(after.len(), before.len(), "one id is still one provider");
        assert_eq!(
            after
                .get(existing.id)
                .map(|one| &*one.manifest.display_name),
            Some("Mine")
        );
        assert_eq!(after.shadowed().len(), 1, "and the shadowing is reported");
    }

    #[test]
    fn a_broken_file_the_operator_wrote_does_not_take_away_the_built_ins() {
        // A mistyped key in one file must not cost the operator every provider they have.
        let scratch = Scratch::make("broken");
        let (home, before) = registry_of(&scratch);

        let path = home
            .paths()
            .providers()
            .join("broken.toml")
            .expect("a valid file name");
        std::fs::write(path.as_std_path(), "this is not toml at all").expect("write it");

        let after = load(&home, runtrol_drivers::builtin());
        assert_eq!(after.len(), before.len(), "the built-ins still work");
        assert_eq!(after.rejected().len(), 1, "and the bad file is reported");
    }

    #[test]
    fn the_kernels_table_carries_which_kinds_are_served_and_never_how() {
        // The seam. The kernel decides whether a kind is served; the crate that ships drivers decides how. The
        // conversion is the constructor being dropped, and that is the whole difference between the two tables.
        let served = runtrol_drivers::builtin();
        let kinds = KindTable::new(
            served
                .kinds
                .iter()
                .map(|entry| KindEntry {
                    kind: entry.kind,
                    unavailable: entry.unavailable,
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(
            kinds.len(),
            served.kinds.len(),
            "no kind is lost crossing over"
        );
        let printed = format!("{kinds:?}");
        assert!(
            !printed.contains("make") && !printed.contains("fn"),
            "the kernel's table must carry no way to build anything: {printed}"
        );
    }

    #[test]
    fn a_kind_this_build_cannot_serve_is_still_listed_with_its_reason() {
        // An operator with a perfectly good manifest for a kind this build has no driver for should see it marked, not
        // wonder where it went.
        let served = runtrol_drivers::builtin();
        let unserved = served
            .kinds
            .iter()
            .find(|entry| entry.make.is_none())
            .expect("this build knows kinds it cannot serve");

        let scratch = Scratch::make("unserved");
        let (home, _) = registry_of(&scratch);
        let path = home
            .paths()
            .providers()
            .join("theirs.toml")
            .expect("a valid file name");
        std::fs::write(
            path.as_std_path(),
            format!(
                "schema = 1\nid = \"theirs\"\ndisplay_name = \"Theirs\"\nkind = \"{}\"\n[bin]\nnames = [\"theirs\"]\n",
                unserved.kind
            ),
        )
        .expect("write it");

        let registry = load(&home, served);
        let theirs = registry
            .get(runtrol_provider::ProviderId::parse("theirs").expect("valid"))
            .expect("it is listed");
        assert!(!theirs.is_usable());
        match theirs.kind {
            runtrol_core::KindStatus::Unavailable { why } => assert!(!why.is_empty()),
            ref other => panic!("expected a named unavailability, got {other:?}"),
        }
    }

    #[test]
    fn composing_starts_no_process() {
        // Measured: a cold start of one of these CLIs costs 300 to 900 ms before it prints anything. Probing every
        // provider here would put a second of nothing in front of the operator's first list.
        let scratch = Scratch::make("noprocess");
        let began = std::time::Instant::now();
        let (_home, registry) = registry_of(&scratch);
        let took = began.elapsed();

        assert!(!registry.is_empty());
        assert!(
            took < std::time::Duration::from_millis(250),
            "composing took {took:?}, which is long enough to be a process start"
        );
    }
}
