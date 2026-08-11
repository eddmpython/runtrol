//! Filesystem identity for one agent workspace.
//!
//! A path is not a writer identity by itself. Two subdirectories can belong to one Git working tree, while two
//! linked working trees can belong to one repository without sharing writable files. This module resolves those
//! cases before the single session owner admits a process. It never runs a provider command and never reads provider
//! state.

use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use runtrol_provider::{AbsPath, PathError, WorkspaceAccess};

const MAX_GIT_POINTER_BYTES: u64 = 4_096;

/// Repository metadata could not be resolved without guessing about filesystem identity.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProjectError {
    /// The filesystem refused a metadata operation.
    #[error("cannot inspect repository metadata at {path:?}: {detail}")]
    Inspect {
        /// The path being inspected.
        path: PathBuf,
        /// What the filesystem reported.
        detail: String,
    },
    /// A Git pointer file exceeded the fixed read budget.
    #[error("repository metadata at {path:?} exceeds {limit} bytes")]
    PointerTooLarge {
        /// The oversized file.
        path: PathBuf,
        /// The read ceiling.
        limit: u64,
    },
    /// A Git pointer file was not UTF-8.
    #[error("repository metadata at {path:?} is not UTF-8")]
    PointerNotUtf8 {
        /// The unreadable file.
        path: PathBuf,
    },
    /// A `.git` file did not contain the required `gitdir:` record.
    #[error("repository metadata at {path:?} has no gitdir pointer")]
    MissingGitDir {
        /// The malformed file.
        path: PathBuf,
    },
    /// A resolved repository path violated the absolute UTF-8 path contract.
    #[error(transparent)]
    Path(#[from] PathError),
}

/// The stable filesystem identity behind one requested workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectIdentity {
    workspace: AbsPath,
    worktree: AbsPath,
    common_store: AbsPath,
}

impl ProjectIdentity {
    /// Resolve a canonical workspace into its working tree and repository group.
    ///
    /// A non-Git directory is its own working tree and project group. For Git, the closest `.git` ancestor owns the
    /// working tree. Linked worktrees resolve `commondir`, so they share a project group without sharing a writer
    /// identity.
    ///
    /// # Errors
    ///
    /// [`ProjectError`] when repository metadata exists but cannot be read or resolved safely.
    pub fn discover(workspace: AbsPath) -> Result<Self, ProjectError> {
        for ancestor in workspace.as_std_path().ancestors() {
            let marker = ancestor.join(".git");
            let Some(metadata) = metadata_if_present(&marker)? else {
                continue;
            };
            let worktree = AbsPath::from_os(ancestor)?;
            let git_dir = if metadata.is_dir() {
                canonical_path(&marker)?
            } else if metadata.is_file() {
                let pointer = read_bounded(&marker)?;
                let Some(value) = pointer.trim().strip_prefix("gitdir:") else {
                    return Err(ProjectError::MissingGitDir { path: marker });
                };
                canonical_join(ancestor, value.trim())?
            } else {
                continue;
            };
            let common_store = resolve_common_store(&git_dir)?;
            return Ok(Self {
                workspace,
                worktree,
                common_store,
            });
        }
        Ok(Self {
            worktree: workspace.clone(),
            common_store: workspace.clone(),
            workspace,
        })
    }

    /// The exact canonical directory requested by the surface.
    #[must_use]
    pub const fn workspace(&self) -> &AbsPath {
        &self.workspace
    }

    /// The writable tree whose files this session can modify.
    #[must_use]
    pub const fn worktree(&self) -> &AbsPath {
        &self.worktree
    }

    /// The repository group shared by linked worktrees.
    #[must_use]
    pub const fn common_store(&self) -> &AbsPath {
        &self.common_store
    }

    /// Whether two identities can name overlapping writable files.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.worktree.is_under(&other.worktree) || other.worktree.is_under(&self.worktree)
    }
}

/// One atomic admission claim handed to the single session owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceClaim {
    identity: ProjectIdentity,
    access: WorkspaceAccess,
}

impl WorkspaceClaim {
    /// Resolve and bind one requested workspace to its access decision.
    ///
    /// # Errors
    ///
    /// [`ProjectError`] when the workspace identity cannot be established safely.
    pub fn discover(workspace: AbsPath, access: WorkspaceAccess) -> Result<Self, ProjectError> {
        Ok(Self {
            identity: ProjectIdentity::discover(workspace)?,
            access,
        })
    }

    /// Build a claim from an already resolved identity.
    #[must_use]
    pub const fn new(identity: ProjectIdentity, access: WorkspaceAccess) -> Self {
        Self { identity, access }
    }

    /// The resolved identity.
    #[must_use]
    pub const fn identity(&self) -> &ProjectIdentity {
        &self.identity
    }

    /// The operator's access decision.
    #[must_use]
    pub const fn access(&self) -> WorkspaceAccess {
        self.access
    }

    /// Consume the claim and return its resolved identity.
    #[must_use]
    pub fn into_identity(self) -> ProjectIdentity {
        self.identity
    }
}

fn metadata_if_present(path: &Path) -> Result<Option<Metadata>, ProjectError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(inspect(path, &error)),
    }
}

fn read_bounded(path: &Path) -> Result<String, ProjectError> {
    let file = File::open(path).map_err(|error| inspect(path, &error))?;
    let mut bytes = Vec::new();
    file.take(MAX_GIT_POINTER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| inspect(path, &error))?;
    if bytes.len() as u64 > MAX_GIT_POINTER_BYTES {
        return Err(ProjectError::PointerTooLarge {
            path: path.to_owned(),
            limit: MAX_GIT_POINTER_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| ProjectError::PointerNotUtf8 {
        path: path.to_owned(),
    })
}

fn resolve_common_store(git_dir: &AbsPath) -> Result<AbsPath, ProjectError> {
    let pointer = git_dir.as_std_path().join("commondir");
    match metadata_if_present(&pointer)? {
        Some(metadata) if metadata.is_file() => {
            let value = read_bounded(&pointer)?;
            canonical_join(git_dir.as_std_path(), value.trim())
        }
        Some(_) => Err(ProjectError::Inspect {
            path: pointer,
            detail: "commondir is not a file".to_owned(),
        }),
        None => Ok(git_dir.clone()),
    }
}

fn canonical_join(base: &Path, value: &str) -> Result<AbsPath, ProjectError> {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    };
    canonical_path(&joined)
}

fn canonical_path(path: &Path) -> Result<AbsPath, ProjectError> {
    let canonical = std::fs::canonicalize(path).map_err(|error| inspect(path, &error))?;
    Ok(AbsPath::from_os(&canonical)?)
}

fn inspect(path: &Path, error: &std::io::Error) -> ProjectError {
    ProjectError::Inspect {
        path: path.to_owned(),
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-project-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous scratch directory");
            }
            std::fs::create_dir_all(&root).expect("create the scratch directory");
            Self { root }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        fn canonical(&self, relative: &str) -> AbsPath {
            AbsPath::canonicalize(
                self.path(relative)
                    .to_str()
                    .expect("the temporary path is UTF-8"),
            )
            .expect("the scratch path is canonical")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::NotFound,
                    "remove the scratch directory: {error}"
                );
            }
        }
    }

    #[test]
    fn subdirectories_in_one_worktree_share_one_writer_identity() {
        let scratch = Scratch::new("one-worktree");
        std::fs::create_dir_all(scratch.path("repo/.git")).expect("create repository metadata");
        std::fs::create_dir_all(scratch.path("repo/a")).expect("create first workspace");
        std::fs::create_dir_all(scratch.path("repo/b")).expect("create second workspace");

        let first = ProjectIdentity::discover(scratch.canonical("repo/a")).expect("discover first");
        let second =
            ProjectIdentity::discover(scratch.canonical("repo/b")).expect("discover second");

        assert_eq!(first.worktree(), second.worktree());
        assert_eq!(first.common_store(), second.common_store());
        assert!(first.overlaps(&second));
    }

    #[test]
    fn linked_worktrees_share_a_project_without_sharing_writer_identity() {
        let scratch = Scratch::new("linked-worktrees");
        std::fs::create_dir_all(scratch.path("main/.git/worktrees/linked"))
            .expect("create linked metadata");
        std::fs::create_dir_all(scratch.path("linked/src")).expect("create linked worktree");
        std::fs::write(
            scratch.path("linked/.git"),
            format!(
                "gitdir: {}\n",
                scratch
                    .path("main/.git/worktrees/linked")
                    .to_str()
                    .expect("the temporary path is UTF-8")
            ),
        )
        .expect("write gitdir pointer");
        std::fs::write(
            scratch.path("main/.git/worktrees/linked/commondir"),
            "../..\n",
        )
        .expect("write common directory pointer");

        let main = ProjectIdentity::discover(scratch.canonical("main")).expect("discover main");
        let linked = ProjectIdentity::discover(scratch.canonical("linked/src"))
            .expect("discover linked worktree");

        assert_eq!(main.common_store(), linked.common_store());
        assert_ne!(main.worktree(), linked.worktree());
        assert!(!main.overlaps(&linked));
    }

    #[test]
    fn non_repository_ancestors_overlap_but_sibling_prefixes_do_not() {
        let scratch = Scratch::new("plain-folders");
        std::fs::create_dir_all(scratch.path("work/repo/src")).expect("create nested workspace");
        std::fs::create_dir_all(scratch.path("work/repo-copy")).expect("create sibling workspace");

        let parent = ProjectIdentity::discover(scratch.canonical("work/repo")).expect("parent");
        let child = ProjectIdentity::discover(scratch.canonical("work/repo/src")).expect("child");
        let sibling =
            ProjectIdentity::discover(scratch.canonical("work/repo-copy")).expect("sibling");

        assert!(parent.overlaps(&child));
        assert!(!parent.overlaps(&sibling));
    }

    #[test]
    fn oversized_git_pointer_is_refused_at_the_read_budget() {
        let scratch = Scratch::new("pointer-budget");
        std::fs::create_dir_all(scratch.path("repo")).expect("create workspace");
        std::fs::write(
            scratch.path("repo/.git"),
            vec![
                b'x';
                usize::try_from(MAX_GIT_POINTER_BYTES + 1)
                    .expect("the fixed pointer budget fits usize")
            ],
        )
        .expect("write oversized pointer");

        assert!(matches!(
            ProjectIdentity::discover(scratch.canonical("repo")),
            Err(ProjectError::PointerTooLarge { .. })
        ));
    }
}
