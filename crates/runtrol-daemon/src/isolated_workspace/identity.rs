//! Captured filesystem objects behind a terminal's immutable project binding.

use runtrol_core::project::ProjectIdentity;
use runtrol_provider::AbsPath;
use runtrol_security::ProjectRootIdentity;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Directory {
    pub(super) path: AbsPath,
    pub(super) identity: [u8; 24],
}

impl Directory {
    pub(super) fn read(path: AbsPath) -> Result<Self, String> {
        let identity = ProjectRootIdentity::read(&path)
            .map_err(|e| e.to_string())?
            .to_bytes();
        Ok(Self { path, identity })
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        let current = AbsPath::canonicalize(self.path.as_str()).map_err(|e| e.to_string())?;
        if current != self.path
            || ProjectRootIdentity::read(&current)
                .map_err(|e| e.to_string())?
                .to_bytes()
                != self.identity
        {
            return Err("the owned directory changed filesystem identity".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedProject {
    pub(super) root: Directory,
    pub(super) store: Directory,
}

impl VerifiedProject {
    pub(crate) fn discover(path: &AbsPath) -> Result<Self, String> {
        let identity = ProjectIdentity::discover(
            AbsPath::canonicalize(path.as_str()).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if identity.common_store() == identity.worktree() {
            return Err("a Git checkout is required".to_owned());
        }
        Ok(Self {
            root: Directory::read(identity.worktree().clone())?,
            store: Directory::read(identity.common_store().clone())?,
        })
    }

    pub(crate) const fn root(&self) -> &AbsPath {
        &self.root.path
    }
    pub(crate) const fn root_identity(&self) -> [u8; 24] {
        self.root.identity
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        self.root.verify()?;
        self.store.verify()?;
        let identity =
            ProjectIdentity::discover(self.root.path.clone()).map_err(|e| e.to_string())?;
        if identity.worktree() != &self.root.path || identity.common_store() != &self.store.path {
            return Err("the captured project changed Git ownership".to_owned());
        }
        Ok(())
    }
}
