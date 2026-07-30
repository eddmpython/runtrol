//! Where runtrol keeps its own files, decided once per process.
//!
//! # The order the root is chosen in
//!
//! 1. `RUNTROL_HOME`, when the operator set it. An explicit choice wins, and it is what makes a test
//!    (or a second instance) able to run against a directory that is not the operator's.
//! 2. Otherwise the platform's own directory for per-user state, plus runtrol's folder inside it.
//!    See [`os`] for the three rules.
//!
//! The result is created, canonicalized, and then never recomputed: [`RuntrolHome`] is resolved at
//! startup and passed by reference from then on.
//!
//! # Why canonical
//!
//! The root's text is load-bearing twice over. It decides whether a workspace lies inside a
//! permitted root, and on Windows it decides the pipe name the CLI and the daemon must agree on. A
//! symbolic link, a short (8.3) filename, or a different case spelling is the same directory to the
//! OS and different text to us, so the text is settled with the filesystem once, here.
//!
//! # What is not here
//!
//! No conversation, and nothing a person reads. This directory holds a pointer per session and
//! runtrol's own bookkeeping. Deleting it costs the operator their labels and pins and nothing else,
//! because every session it points at is still openable with the provider's own resume command. That
//! property is what makes the directory safe to delete, and it is checked as an acceptance test
//! rather than promised in prose.

pub mod layout;
mod os;

use std::io;

use runtrol_provider::{AbsPath, PathError};

pub use layout::{Endpoint, Layout};
pub use os::Ignored;
use os::Unusable;

/// The variable an operator sets to put runtrol's files somewhere of their choosing.
pub const HOME_ENV: &str = "RUNTROL_HOME";

/// runtrol's folder inside whatever directory the platform designates.
const FOLDER: &str = "runtrol";

/// runtrol's home directory could not be established.
///
/// Every variant names the thing the operator has to change. A home directory that cannot be
/// resolved stops the process before it does anything else, so this is the first error a person can
/// see and it has to be the one that tells them where to look.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HomeError {
    /// An environment variable this platform needs does not name a directory.
    #[error("{name} {why}")]
    EnvUnusable {
        /// The variable's name.
        name: &'static str,
        /// What is wrong with it, phrased to follow the name.
        why: &'static str,
    },

    /// An environment variable named something that is not a usable absolute path.
    #[error("{name} is set to {value:?}, which cannot be used: {source}")]
    BadEnv {
        /// The variable's name.
        name: &'static str,
        /// What it was set to.
        value: String,
        /// Why the path was refused.
        source: PathError,
    },

    /// The home directory itself is not a usable absolute path.
    #[error("cannot use {given:?} as a home directory: {source}")]
    Root {
        /// The directory as given.
        given: String,
        /// Why it was refused.
        source: PathError,
    },

    /// A path inside the home directory could not be formed.
    ///
    /// The segments are constants in [`layout`], so this cannot fire as the code stands. It is an
    /// error rather than a panic because a constant is exactly the kind of thing a later edit
    /// changes, and a supervisor that aborts on a bad constant is worse than one that reports it.
    #[error("cannot form the path for {segment:?} inside the home directory: {source}")]
    Layout {
        /// The segment that would not join.
        segment: &'static str,
        /// Why it was refused.
        source: PathError,
    },

    /// The socket path would not fit the kernel's address field.
    ///
    /// Unix only: the address is a fixed-size array there, whereas a Windows pipe name is a
    /// fingerprint of bounded length and cannot run over.
    #[cfg(unix)]
    #[error("the socket path {path} is longer than this kernel's limit of {limit} bytes")]
    SocketPathTooLong {
        /// The path that is too long.
        path: AbsPath,
        /// How many bytes of path the kernel accepts.
        limit: usize,
    },

    /// A directory could not be created.
    ///
    /// The OS error is flattened to a kind and its own words, matching [`PathError::Resolve`], so
    /// this type stays comparable and a caller can tell "permission denied" from "the disk is full"
    /// without reading the message.
    #[error("cannot create {path}: {detail}")]
    Create {
        /// The directory that could not be created.
        path: String,
        /// What class of failure the OS reported.
        kind: io::ErrorKind,
        /// What the OS said, verbatim.
        detail: String,
    },
}

/// runtrol's home directory, resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RuntrolHome {
    /// Every path inside it.
    paths: Layout,
    /// An operator setting that was not usable, if there was one.
    ignored: Option<Ignored>,
}

impl RuntrolHome {
    /// Resolve the home directory for this machine and this operator, creating it if needed.
    ///
    /// # Errors
    ///
    /// Any [`HomeError`]: the environment does not name a base directory, the path it names is not
    /// usable, or a directory could not be created.
    pub fn open() -> Result<Self, HomeError> {
        let (root, ignored) = choose(&os::looked_up)?;
        Self::assemble(&root, ignored)
    }

    /// Resolve a home directory at a given root, creating it if needed.
    ///
    /// This is what a `--home` argument and a test both use. It exists so that starting runtrol
    /// against a directory that is not the operator's needs no environment surgery.
    ///
    /// # Errors
    ///
    /// [`HomeError::Root`] when the path is not usable, [`HomeError::Create`] when it cannot be
    /// created, plus the layout errors.
    pub fn open_at(root: &str) -> Result<Self, HomeError> {
        Self::assemble(root, None)
    }

    /// Every path inside the home directory.
    #[must_use]
    pub const fn paths(&self) -> &Layout {
        &self.paths
    }

    /// An environment setting that was ignored while resolving this home, if any.
    ///
    /// Something to put in front of a person once, at startup. An operator who set a variable that
    /// had no effect will otherwise look for their sessions in the wrong directory.
    #[must_use]
    pub const fn ignored(&self) -> Option<&Ignored> {
        self.ignored.as_ref()
    }

    /// Create the directory tree and settle every path inside it.
    fn assemble(root: &str, ignored: Option<Ignored>) -> Result<Self, HomeError> {
        let declared = AbsPath::new(root).map_err(|source| HomeError::Root {
            given: root.to_owned(),
            source,
        })?;
        create(&declared)?;

        // Canonical only after creating it: resolving a path asks the filesystem, and the filesystem
        // has no answer for a directory that is not there yet.
        let canonical =
            AbsPath::canonicalize(declared.as_str()).map_err(|source| HomeError::Root {
                given: declared.as_str().to_owned(),
                source,
            })?;

        let paths = Layout::resolve(canonical)?;
        for directory in paths.directories() {
            create(directory)?;
        }
        Ok(Self { paths, ignored })
    }
}

/// Decide which directory is the home, and report any setting that was passed over.
///
/// Takes the lookup rather than reading the environment, for the same reason [`os`] does: setting a
/// variable is `unsafe` and process-global, so the only way to test the choice is to be able to hand
/// it an environment. The shipping call and the tested call are then the same code.
fn choose(
    look: &impl Fn(&str) -> Result<String, Unusable>,
) -> Result<(String, Option<Ignored>), HomeError> {
    match look(HOME_ENV) {
        // An explicit choice, used exactly as given.
        Ok(chosen) => Ok((chosen, None)),

        // Set to bytes that are not UTF-8. Falling back would mean writing somewhere other than
        // where the operator said, which is the one thing an explicit setting must never do.
        Err(Unusable::NotUtf8) => Err(HomeError::EnvUnusable {
            name: HOME_ENV,
            why: Unusable::NotUtf8.detail(),
        }),

        // Unset, or set to nothing. The operator has expressed no choice, so the platform's rule
        // decides. An empty value is how a shell spells "unset" by accident, and there is nothing
        // for anyone to correct.
        Err(Unusable::Unset | Unusable::Empty) => {
            let base = os::base_from(look)?;
            let root = base.path.join(FOLDER).map_err(|source| HomeError::Layout {
                segment: FOLDER,
                source,
            })?;
            Ok((root.as_str().to_owned(), base.ignored))
        }
    }
}

/// Create a directory and every parent it needs.
fn create(path: &AbsPath) -> Result<(), HomeError> {
    std::fs::create_dir_all(path.as_std_path()).map_err(|source| HomeError::Create {
        path: path.as_str().to_owned(),
        kind: source.kind(),
        detail: source.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when the test ends.
    struct Scratch {
        /// Where it is, as text, because the point is to hand it to [`RuntrolHome::open_at`].
        root: String,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("runtrol-home-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            Self {
                root: root
                    .to_str()
                    .expect("the temporary directory must be UTF-8")
                    .to_owned(),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                // The test has already made its assertions; a directory left in the temporary folder
                // is worth a line, not a failure.
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    #[test]
    fn opening_a_home_creates_every_directory_it_names() {
        let scratch = Scratch::make("creates");
        let home = RuntrolHome::open_at(&scratch.root).expect("a fresh home must open");

        assert!(home.paths().root().as_std_path().is_dir());
        for directory in home.paths().directories() {
            assert!(
                directory.as_std_path().is_dir(),
                "{directory:?} was named and not created"
            );
        }
        // Files are not created up front. Their owners write them, and an empty database file would
        // be a file the storage engine has to reject rather than create.
        assert!(!home.paths().database().as_std_path().exists());
    }

    #[test]
    fn opening_twice_is_the_same_home() {
        let scratch = Scratch::make("twice");
        let first = RuntrolHome::open_at(&scratch.root).expect("first open");
        let second = RuntrolHome::open_at(&scratch.root).expect("second open");
        assert_eq!(first, second);
    }

    #[test]
    fn deleting_the_home_and_starting_again_works() {
        // The acceptance property behind "runtrol holds nothing of yours hostage": the directory is
        // safe to delete, so starting after deleting it has to be an ordinary start.
        let scratch = Scratch::make("deleted");
        let before = RuntrolHome::open_at(&scratch.root).expect("first open");
        std::fs::remove_dir_all(before.paths().root().as_std_path()).expect("delete the home");

        let after = RuntrolHome::open_at(&scratch.root).expect("a deleted home must reopen");
        assert_eq!(before, after);
        for directory in after.paths().directories() {
            assert!(directory.as_std_path().is_dir());
        }
    }

    #[test]
    fn a_relative_home_is_refused_by_name() {
        // Resolving it would depend on the daemon's working directory, which the operator did not
        // choose and cannot see.
        match RuntrolHome::open_at("relative/home") {
            Err(HomeError::Root { given, .. }) => assert_eq!(given, "relative/home"),
            other => panic!("expected a refusal naming the path, got {other:?}"),
        }
    }

    #[test]
    fn the_resolved_root_is_canonical() {
        // The root's text decides the endpoint address and every "is this inside the workspace"
        // answer, so two spellings of one directory have to arrive as one.
        let scratch = Scratch::make("canonical");
        let home = RuntrolHome::open_at(&scratch.root).expect("open");
        let root = home.paths().root();

        let resolved =
            AbsPath::canonicalize(root.as_str()).expect("an existing directory resolves");
        assert_eq!(root, &resolved);
    }

    #[cfg(windows)]
    #[test]
    fn a_differently_cased_spelling_resolves_to_the_one_the_filesystem_uses() {
        // Windows opens the same directory for either spelling. If the spelling survived, the pipe
        // name would differ, and a CLI started from a shortcut with one spelling would fail to find
        // a daemon started from a shell with the other.
        let scratch = Scratch::make("cased");
        std::fs::create_dir_all(&scratch.root).expect("create the home with its real spelling");
        let shouted = scratch.root.to_ascii_uppercase();

        let home = RuntrolHome::open_at(&shouted).expect("a shouted spelling must open");
        let expected = AbsPath::canonicalize(&scratch.root).expect("the real spelling resolves");
        assert_eq!(home.paths().root(), &expected);
        assert_ne!(
            home.paths().root().as_str(),
            shouted,
            "the spelling as typed must not survive resolution"
        );
        assert_eq!(
            home,
            RuntrolHome::open_at(&scratch.root).expect("open"),
            "two spellings of one directory must be one home"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_resolves_to_its_target() {
        // Same property, tested with the mechanism Unix actually has. A link that stayed a link would
        // give the same directory two identities, and "is this workspace inside the home" would
        // answer differently depending on which one the caller happened to hold.
        let scratch = Scratch::make("symlink");
        let target = format!("{}/real", scratch.root);
        let link = format!("{}/link", scratch.root);
        std::fs::create_dir_all(&target).expect("create the target");
        std::os::unix::fs::symlink(&target, &link).expect("create the link");

        let home = RuntrolHome::open_at(&link).expect("opening through a link must work");
        let expected = AbsPath::canonicalize(&target).expect("the target resolves");
        assert_eq!(home.paths().root(), &expected);
        assert_ne!(
            home.paths().root().as_str(),
            link,
            "the link must not survive resolution"
        );
    }

    /// A lookup that answers from a list, so the choice can be exercised without touching anything.
    fn fake(entries: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, Unusable> + use<> {
        let owned: Vec<(String, String)> = entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            let found = owned
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone());
            match found {
                None => Err(Unusable::Unset),
                Some(value) if value.is_empty() => Err(Unusable::Empty),
                Some(value) => Ok(value),
            }
        }
    }

    /// A platform-shaped environment with no runtrol setting in it.
    fn platform_defaults() -> Vec<(&'static str, &'static str)> {
        if cfg!(windows) {
            vec![("LOCALAPPDATA", r"C:\Users\someone\AppData\Local")]
        } else {
            vec![("HOME", "/home/someone"), ("XDG_STATE_HOME", "/state")]
        }
    }

    #[test]
    fn an_explicit_setting_wins_over_the_platform_rule() {
        // The operator said where. Nothing may second-guess that, including us.
        let mut environment = platform_defaults();
        let chosen = if cfg!(windows) {
            r"D:\elsewhere\runtrol"
        } else {
            "/elsewhere/runtrol"
        };
        environment.push((HOME_ENV, chosen));

        let (root, ignored) =
            choose(&fake(&environment)).expect("an explicit setting must resolve");
        assert_eq!(root, chosen);
        assert_eq!(ignored, None);
    }

    #[test]
    fn an_explicit_setting_that_cannot_be_read_stops_the_start() {
        // Falling back here would write to a directory the operator did not choose, and they would
        // then find an empty session list with nothing anywhere saying why. Refusing to start is the
        // only answer that leaves them able to fix it.
        let look = |name: &str| {
            if name == HOME_ENV {
                Err(Unusable::NotUtf8)
            } else {
                Ok("/somewhere".to_owned())
            }
        };
        match choose(&look) {
            Err(HomeError::EnvUnusable { name, why }) => {
                assert_eq!(name, HOME_ENV);
                assert_eq!(why, Unusable::NotUtf8.detail());
            }
            other => panic!("expected the start to stop and say why, got {other:?}"),
        }
    }

    #[test]
    fn with_no_setting_the_folder_goes_under_the_platform_directory() {
        let (root, ignored) =
            choose(&fake(&platform_defaults())).expect("the platform rule must resolve");
        let path = AbsPath::new(&root).expect("the chosen root must be a usable path");
        assert_eq!(path.file_name(), Some(FOLDER));
        assert_eq!(ignored, None);
    }

    #[test]
    fn the_choice_on_this_machine_resolves_with_no_configuration_at_all() {
        // A first run has no settings. This asserts the default path is reachable here without
        // creating anything, because a unit test has no business writing to the operator's own
        // state directory.
        let (root, _) = choose(&os::looked_up).expect("a first run must have a home to go to");
        assert!(
            AbsPath::new(&root).is_ok(),
            "{root:?} is not a usable absolute path"
        );
    }

    #[test]
    fn the_error_for_a_bad_variable_says_the_variable_and_the_reason() {
        let error = HomeError::EnvUnusable {
            name: HOME_ENV,
            why: Unusable::NotUtf8.detail(),
        };
        let message = error.to_string();
        assert!(message.starts_with(HOME_ENV), "{message}");
        assert!(message.contains("UTF-8"), "{message}");
    }
}
