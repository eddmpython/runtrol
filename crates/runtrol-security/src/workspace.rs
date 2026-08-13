//! Where work is allowed to happen, and the directories no configuration may open.
//!
//! An approved directory tree is a [`WorkspaceRoot`]. `runtrol-core` accepts one of these when
//! starting a session, never a bare path, so a path that never passed through here cannot reach a
//! spawn.
//!
//! # The deny rule, in one sentence
//!
//! A candidate root is refused when it overlaps a denied directory **in either direction**.
//!
//! The second direction is the one that matters. Refusing a root that sits inside `~/.ssh` is
//! obvious. The mistake people actually make is approving their home directory, which puts every
//! credential store on the machine inside an approved tree. One containment check, run both ways,
//! covers both, and it also makes an explicit rule for the home directory and for filesystem roots
//! unnecessary: they are refused because denied directories live under them.
//!
//! # What this is not
//!
//! It is not a sandbox. A symbolic link inside an approved root, pointing at something denied, is
//! followed by the agent at run time and no check here can see it. Runtime confinement belongs to the
//! provider CLI's own permission system; this module decides what the operator is allowed to hand
//! over in the first place. Saying so plainly is better than implying a containment guarantee that
//! does not exist.

use runtrol_provider::{AbsPath, PathError};

use crate::error::SecurityError;
use crate::id::WorkspaceRootId;
use crate::root_identity::ProjectRootIdentity;

/// A directory no configuration may open, and why.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeniedPath {
    /// Path relative to the operator's home directory, using `/` as the separator.
    pub under_home: &'static str,
    /// Why it is denied. Reaches the operator verbatim when a root is refused.
    pub why: &'static str,
}

/// Directories inside the operator's home that no workspace root may overlap, whatever this build drives.
///
/// Every entry is a place a coding agent could read a credential from or write one to. The list is
/// not exhaustive and cannot be: a tool released next month will put a token somewhere new. It covers
/// what is both common and catastrophic, and the honest limit is stated in the module documentation.
///
/// Not configurable. A deny list the operator can edit is a deny list a prompt injection can talk
/// them into editing, and the whole point of these entries is that they hold when judgement fails.
///
/// # What is not here
///
/// Where each provider keeps its own login. That is declared by the provider's manifest and reaches this list
/// through [`DenyList::new`], so a provider added by shipping a manifest is protected with nothing in this crate
/// to edit. The three coding CLIs named below are the exception and say why at the line: they are protected even
/// on a build that cannot drive them, because an agent runtrol did start can read another CLI's credentials.
const DENIED_UNDER_HOME: &[DeniedPath] = &[
    DeniedPath {
        under_home: ".ssh",
        why: "private keys that authenticate you to every machine you use",
    },
    DeniedPath {
        under_home: ".gnupg",
        why: "signing and encryption keys",
    },
    DeniedPath {
        under_home: ".aws",
        why: "cloud credentials that can spend money",
    },
    DeniedPath {
        under_home: ".azure",
        why: "cloud credentials that can spend money",
    },
    DeniedPath {
        under_home: ".config/gcloud",
        why: "cloud credentials that can spend money",
    },
    DeniedPath {
        under_home: ".kube",
        why: "cluster credentials",
    },
    DeniedPath {
        under_home: ".docker",
        why: "registry credentials",
    },
    DeniedPath {
        under_home: ".config/gh",
        why: "a token that can push to your repositories",
    },
    DeniedPath {
        under_home: ".git-credentials",
        why: "stored git passwords",
    },
    DeniedPath {
        under_home: ".netrc",
        why: "stored passwords for anything that speaks it",
    },
    DeniedPath {
        under_home: ".npmrc",
        why: "a publish token for the package registry",
    },
    DeniedPath {
        under_home: ".cargo/credentials.toml",
        why: "a publish token for the package registry",
    },
    DeniedPath {
        under_home: ".pypirc",
        why: "a publish token for the package registry",
    },
    // The coding CLIs runtrol supervises keep their own subscription credentials here. runtrol never
    // holds a provider credential, and it must not approve a workspace that would let an agent read
    // one either.
    // provider-name: a credential directory, not a provider this crate knows about. Every provider's own
    // login arrives from its manifest instead, so nothing here has to change to cover a new one. These three
    // stay because the agent that reads them is not the one they belong to: a session runtrol did start can
    // walk into another CLI's directory, whether or not this build can drive that CLI.
    DeniedPath {
        under_home: ".claude",
        why: "a coding CLI's own login, which runtrol deliberately never holds",
    },
    // provider-name: see above.
    DeniedPath {
        under_home: ".codex",
        why: "a coding CLI's own login, which runtrol deliberately never holds",
    },
    // provider-name: see above.
    DeniedPath {
        under_home: ".gemini",
        why: "a coding CLI's own login, which runtrol deliberately never holds",
    },
];

/// The set of directories a workspace root may not overlap.
///
/// Built once, at startup, from two paths the daemon resolves and passes in: the operator's home, and
/// runtrol's own state directory. Taking them as arguments is what keeps this crate a leaf, with no
/// opinion about per-platform directory layout.
#[derive(Clone, Debug)]
pub struct DenyList {
    /// Resolved denied paths, paired with the reason for each.
    ///
    /// The reason is owned rather than borrowed, because some of these come from a provider's manifest and a
    /// manifest is read at runtime. One representation for both sources: two would mean two ways to be a denied
    /// path, and the one nobody thought about would be the one missing from a check.
    entries: Vec<(AbsPath, Box<str>)>,
}

impl DenyList {
    /// Build the list for this machine.
    ///
    /// `state_dir` is where runtrol keeps its database and its grant record. It is denied for a
    /// different reason than the rest: an agent that can write there can rewrite the permissions that
    /// were meant to contain it.
    ///
    /// Each denied path is resolved through the filesystem when it exists, and kept as written when it
    /// does not. Resolving matters because the candidate root is also resolved, and comparing a
    /// resolved path against an unresolved one answers the wrong question. Absent paths still count:
    /// `~/.aws` not existing today does not mean it will not exist when the agent runs.
    ///
    /// # Errors
    ///
    /// [`PathError`] when an entry cannot be joined onto `home`, which means either a constant in this file
    /// or a provider manifest is malformed.
    ///
    /// Fallible on purpose. The alternative is skipping the entry that failed, and a deny list with a
    /// silently missing row is a hole exactly where holes are worst. A list that cannot be built in
    /// full stops the daemon from starting, which the operator sees and can act on.
    pub fn new(home: &AbsPath, state_dir: &AbsPath, declared: &[&str]) -> Result<Self, PathError> {
        let mut entries: Vec<(AbsPath, Box<str>)> =
            Vec::with_capacity(DENIED_UNDER_HOME.len() + declared.len() + 1);

        entries.push((
            resolve_or_keep(state_dir),
            "runtrol's own state, including the record of what you have permitted".into(),
        ));

        for denied in DENIED_UNDER_HOME {
            let path = home.join(denied.under_home)?;
            entries.push((resolve_or_keep(&path), denied.why.into()));
        }

        // Where each installed provider keeps its own login, as its manifest declared. Adding a provider is
        // therefore adding a file, with nothing in this crate to change: the failure this closes is a wall that
        // protects the CLIs somebody thought of and silently misses the one shipped last week.
        for under_home in declared {
            let path = home.join(under_home)?;
            entries.push((
                resolve_or_keep(&path),
                "a coding CLI's own login, which runtrol deliberately never holds".into(),
            ));
        }

        Ok(Self { entries })
    }

    /// The denied path this candidate overlaps, if any.
    ///
    /// Checks containment both ways, which is the whole rule.
    #[must_use]
    pub fn overlap(&self, candidate: &AbsPath) -> Option<(&AbsPath, &str)> {
        self.entries
            .iter()
            .find(|(denied, _)| candidate.is_under(denied) || denied.is_under(candidate))
            .map(|(denied, why)| (denied, &**why))
    }

    /// How many directories are denied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is denied, which should never be true on a real machine.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Resolve through the filesystem, or keep the path as written when it does not exist.
fn resolve_or_keep(path: &AbsPath) -> AbsPath {
    match AbsPath::canonicalize(path.as_str()) {
        Ok(resolved) => resolved,
        // The path does not exist yet, or cannot be resolved. Keeping the literal form is right: it
        // still denies the directory by name, which is what matters for a credential store that has
        // not been created yet. Dropping the entry would be the unsafe direction.
        Err(_) => path.clone(),
    }
}

/// A directory tree the operator has approved as a place work may happen.
///
/// The only way to obtain one is [`WorkspaceRoot::approve`], so a value of this type is proof that a
/// path was canonicalized, confirmed to be a directory, and held against the deny list. Functions that
/// spawn a coding agent take this rather than a path, which moves "was this checked" from a question
/// about call order into a question the compiler answers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorkspaceRoot {
    /// Minted when approved, so removing and re-adding the directory yields a different root.
    id: WorkspaceRootId,
    /// The canonical path.
    path: AbsPath,
    /// The exact directory that occupied the path during approval.
    identity: ProjectRootIdentity,
}

impl WorkspaceRoot {
    /// Check a proposed directory and approve it.
    ///
    /// Three steps, in this order, because each depends on the one before: resolve through the OS,
    /// confirm it is a directory, then hold the resolved path against the deny list.
    ///
    /// # Errors
    ///
    /// [`SecurityError::WorkspaceUnresolvable`] when the OS cannot resolve it,
    /// [`SecurityError::WorkspaceNotADirectory`] when it is a file, and
    /// [`SecurityError::WorkspaceDenied`] when it overlaps something on the deny list, and
    /// [`SecurityError::WorkspaceIdentityUnavailable`] when the OS cannot bind the exact directory.
    pub fn approve(candidate: &str, deny: &DenyList) -> Result<Self, SecurityError> {
        let path = AbsPath::canonicalize(candidate)
            .map_err(|source| SecurityError::WorkspaceUnresolvable { source })?;

        if !path.as_std_path().is_dir() {
            return Err(SecurityError::WorkspaceNotADirectory {
                candidate: candidate.to_owned(),
            });
        }

        if let Some((denied, why)) = deny.overlap(&path) {
            return Err(SecurityError::WorkspaceDenied {
                candidate: path.clone(),
                denied: denied.clone(),
                why: why.into(),
            });
        }

        let identity = ProjectRootIdentity::read(&path).map_err(|source| {
            SecurityError::WorkspaceIdentityUnavailable {
                candidate: path.clone(),
                kind: source.kind(),
                detail: source.to_string(),
            }
        })?;

        Ok(Self {
            id: WorkspaceRootId::now(),
            path,
            identity,
        })
    }

    /// This root's identifier, as it appears in a permission scope.
    #[must_use]
    pub const fn id(&self) -> WorkspaceRootId {
        self.id
    }

    /// The canonical path.
    #[must_use]
    pub const fn path(&self) -> &AbsPath {
        &self.path
    }

    /// The exact filesystem object approved at this path.
    #[must_use]
    pub const fn identity(&self) -> ProjectRootIdentity {
        self.identity
    }

    /// Whether `path` lies inside this root.
    ///
    /// Takes an already-canonical path, because comparing an unresolved path against a resolved root
    /// is how a symbolic link gets through.
    #[must_use]
    pub fn contains(&self, path: &AbsPath) -> bool {
        path.is_under(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory tree standing in for a home directory.
    ///
    /// Real directories rather than a fake filesystem, because `approve` canonicalizes through the OS
    /// and a test that skipped that would not exercise the thing most likely to be wrong.
    struct Home {
        root: AbsPath,
    }

    impl Home {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-workspace-{name}"));
            // Left behind by an interrupted earlier run. Removing it first is what makes the test
            // repeatable.
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the test home");
            let root = AbsPath::canonicalize(base.to_str().expect("temp dir is UTF-8"))
                .expect("canonicalize the test home");
            Self { root }
        }

        fn dir(&self, relative: &str) -> AbsPath {
            let path = self.root.join(relative).expect("valid relative segment");
            std::fs::create_dir_all(path.as_std_path()).expect("create the directory");
            AbsPath::canonicalize(path.as_str()).expect("canonicalize")
        }

        fn file(&self, relative: &str) -> AbsPath {
            let path = self.root.join(relative).expect("valid relative segment");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent.as_std_path()).expect("create the parent");
            }
            std::fs::write(path.as_std_path(), b"x").expect("create the file");
            AbsPath::canonicalize(path.as_str()).expect("canonicalize")
        }

        fn deny_list(&self) -> DenyList {
            let state = self.dir("state");
            // Nothing declared: this fixture is about the constants, and what a manifest adds has its own test.
            DenyList::new(&self.root, &state, &[])
                .expect("the deny list constants must all be joinable")
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // Reported rather than swallowed, and reported rather than panicked: a panic in `drop`
            // during an already-failing test would replace the real failure with this one. The next
            // run's `make` clears a leftover directory, so a failure here cannot make a later test
            // wrong.
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    #[test]
    fn an_ordinary_project_directory_is_approved() {
        let home = Home::make("ordinary");
        let project = home.dir("projects/runtrol");
        let root = WorkspaceRoot::approve(project.as_str(), &home.deny_list())
            .expect("a project directory must be approvable");
        assert_eq!(root.path(), &project);
    }

    #[test]
    fn a_credential_directory_is_refused() {
        let home = Home::make("inside");
        let keys = home.dir(".ssh");
        let error = WorkspaceRoot::approve(keys.as_str(), &home.deny_list())
            .expect_err("a key directory must be refused");
        assert!(matches!(error, SecurityError::WorkspaceDenied { .. }));
    }

    #[test]
    fn a_directory_inside_a_credential_directory_is_refused() {
        let home = Home::make("deeper");
        let nested = home.dir(".ssh/backup");
        assert!(matches!(
            WorkspaceRoot::approve(nested.as_str(), &home.deny_list()),
            Err(SecurityError::WorkspaceDenied { .. })
        ));
    }

    #[test]
    fn the_home_directory_itself_is_refused_because_it_contains_denied_directories() {
        // This is the mistake people actually make, and it is caught by the same containment check
        // running the other way rather than by a rule written specially for it.
        let home = Home::make("home");
        home.dir(".ssh");
        let error = WorkspaceRoot::approve(home.root.as_str(), &home.deny_list())
            .expect_err("approving a home directory hands over every credential inside it");
        match error {
            SecurityError::WorkspaceDenied { denied, .. } => {
                assert!(
                    denied.is_under(&home.root),
                    "the reason names what is inside"
                );
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn runtrols_own_state_directory_is_refused() {
        // An agent that can write here can rewrite the record of what it was permitted to do.
        let home = Home::make("state");
        let state = home.dir("state");
        assert!(matches!(
            WorkspaceRoot::approve(state.as_str(), &home.deny_list()),
            Err(SecurityError::WorkspaceDenied { .. })
        ));
    }

    #[test]
    fn a_denied_path_that_does_not_exist_yet_still_denies() {
        // `~/.aws` not existing today does not mean it will not exist while the agent is running.
        let home = Home::make("absent");
        let deny = home.deny_list();
        let future_credentials = home.root.join(".aws").expect("valid segment");
        assert!(
            deny.overlap(&future_credentials).is_some(),
            "a path is denied by name, not by whether it exists"
        );
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_refused() {
        // `~/.sshkeys-notes` is not `~/.ssh`. A text prefix comparison would confuse the two.
        let home = Home::make("sibling");
        home.dir(".ssh");
        let notes = home.dir(".sshkeys-notes");
        assert!(
            WorkspaceRoot::approve(notes.as_str(), &home.deny_list()).is_ok(),
            "a sibling sharing a text prefix must still be approvable"
        );
    }

    #[test]
    fn a_file_is_not_a_workspace() {
        let home = Home::make("file");
        let readme = home.file("projects/README.md");
        assert!(matches!(
            WorkspaceRoot::approve(readme.as_str(), &home.deny_list()),
            Err(SecurityError::WorkspaceNotADirectory { .. })
        ));
    }

    #[test]
    fn a_path_that_does_not_exist_is_not_a_workspace() {
        let home = Home::make("missing");
        let absent = home.root.join("nope").expect("valid segment");
        assert!(matches!(
            WorkspaceRoot::approve(absent.as_str(), &home.deny_list()),
            Err(SecurityError::WorkspaceUnresolvable { .. })
        ));
    }

    #[test]
    fn every_denied_entry_is_built() {
        // `DenyList::new` skips an entry it cannot join, which would silently shrink the list. This
        // pins the count so a malformed constant shows up here instead of as a missing denial.
        let home = Home::make("count");
        assert_eq!(home.deny_list().len(), DENIED_UNDER_HOME.len() + 1);
        assert!(!home.deny_list().is_empty());
    }

    #[test]
    fn a_directory_a_manifest_declared_is_refused_the_same_as_a_constant() {
        // The reason this is a manifest key. A provider added by shipping a file has to be protected with
        // nothing in this crate to edit, or the wall covers the CLIs somebody thought of and silently misses the
        // one released last week.
        let home = Home::make("declared");
        let secret = home.dir(".somethingnew");
        let state = home.dir("state");

        let without = DenyList::new(&home.root, &state, &[]).expect("built");
        assert!(
            WorkspaceRoot::approve(secret.as_str(), &without).is_ok(),
            "this test needs a directory the constants do not already deny"
        );

        let with = DenyList::new(&home.root, &state, &[".somethingnew"]).expect("built");
        match WorkspaceRoot::approve(secret.as_str(), &with) {
            Err(SecurityError::WorkspaceDenied { why, .. }) => {
                assert!(why.contains("own login"), "{why}");
            }
            other => panic!("a declared secret directory must be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_declared_path_is_denied_in_both_directions_like_every_other_entry() {
        // Coming from a manifest must not make an entry weaker than a constant. The direction that matters is
        // the second one: approving a parent of a credential directory is the mistake people actually make.
        let home = Home::make("declaredBothWays");
        drop(home.dir("work/.somethingnew"));
        let state = home.dir("state");
        let deny = DenyList::new(&home.root, &state, &["work/.somethingnew"]).expect("built");

        let parent = home.dir("work");
        assert!(
            matches!(
                WorkspaceRoot::approve(parent.as_str(), &deny),
                Err(SecurityError::WorkspaceDenied { .. })
            ),
            "a root containing a declared secret directory has to be refused"
        );
    }

    #[test]
    fn approving_the_same_directory_twice_gives_two_different_roots() {
        // Root identifiers are minted, not derived, so removing a root is final.
        let home = Home::make("twice");
        let project = home.dir("projects/app");
        let deny = home.deny_list();
        let first = WorkspaceRoot::approve(project.as_str(), &deny).expect("approved");
        let second = WorkspaceRoot::approve(project.as_str(), &deny).expect("approved");
        assert_eq!(first.path(), second.path());
        assert_eq!(first.identity(), second.identity());
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn replacing_a_directory_at_the_same_path_changes_its_authority_identity() {
        let home = Home::make("replacement");
        let project = home.dir("projects/app");
        let deny = home.deny_list();
        let approved = WorkspaceRoot::approve(project.as_str(), &deny).expect("approved");
        let retired = home
            .root
            .join("projects/retired")
            .expect("valid retired path");
        std::fs::rename(project.as_std_path(), retired.as_std_path()).expect("retire old root");
        std::fs::create_dir(project.as_std_path()).expect("replace root at the same path");
        let replacement = WorkspaceRoot::approve(project.as_str(), &deny).expect("replacement");
        assert_eq!(approved.path(), replacement.path());
        assert_ne!(approved.identity(), replacement.identity());
    }

    #[test]
    fn contains_answers_for_the_tree_and_not_for_a_neighbour() {
        let home = Home::make("contains");
        let project = home.dir("projects/app");
        let inside = home.dir("projects/app/src");
        let outside = home.dir("projects/other");
        let root = WorkspaceRoot::approve(project.as_str(), &home.deny_list()).expect("approved");
        assert!(root.contains(&inside));
        assert!(root.contains(&project), "a root contains itself");
        assert!(!root.contains(&outside));
    }
}
