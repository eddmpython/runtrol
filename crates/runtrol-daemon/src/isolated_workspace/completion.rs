//! Existing worktree rows record positive generation completion, never infer it from a missing PID.

use serde::{Deserialize, Serialize};

use super::ownership::ProcessStamp;

/// Minted after exact process completion while its closing reservation still excludes new birth.
pub(crate) struct EndedSession {
    runtime: ProcessStamp,
    session: runtrol_provider::SessionId,
}

impl EndedSession {
    pub(crate) fn after_close_completed(
        session: runtrol_provider::SessionId,
    ) -> Result<Self, String> {
        let runtime = runtrol_childproc::process_identity(std::process::id())
            .ok_or("the current Runtime cannot be identified")?
            .into();
        Ok(Self { runtime, session })
    }
}

pub(super) fn require_structured_release(record: &super::Record) -> Result<(), String> {
    #[cfg(windows)]
    {
        let proof = record
            .lifetime
            .ok_or("the worktree has no recorded Runtime completion authority")?;
        let current = runtrol_childproc::process_identity(std::process::id())
            .ok_or("the current Runtime cannot be identified")?;
        if proof.runtime != ProcessStamp::from(current) {
            if proof.runtime.is_live() || !proven(Some(proof), proof.runtime) {
                return Err(
                    "the original worktree Runtime has no positive completion proof".to_owned(),
                );
            }
            return Ok(());
        }
        if record.session_id.is_some() && !record.session_completed {
            return Err("the bound worktree session has no exact completed close".to_owned());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _observed = record;
        Ok(())
    }
}

impl super::IsolatedWorkspaceController {
    /// Called off the reactor after the owner has proved the exact session close.
    pub(crate) fn complete_session(&self, ended: &EndedSession) -> Result<(), String> {
        let session = ended.session.to_string();
        let Some(mut record) = super::registry::read(self.path())?
            .into_iter()
            .find(|record| {
                record.terminal.is_none()
                    && record.session_id.as_deref() == Some(session.as_str())
                    && record
                        .lifetime
                        .is_some_and(|proof| proof.runtime == ended.runtime)
            })
        else {
            return Ok(());
        };
        let _operation = super::registry::operation(self.path(), &record.workspace_id)?;
        if record.session_completed || record.state == super::State::Released {
            return Ok(());
        }
        record.session_completed = true;
        super::registry::rollback(self.path(), record)?;
        Ok(())
    }

    /// Invalidate the previous exact close before a reserved session can execute again.
    pub(crate) fn reopen_session(
        &self,
        session: runtrol_provider::SessionId,
        workspace: &runtrol_provider::AbsPath,
    ) -> Result<(), String> {
        let Some(mut record) = super::registry::read(self.path())?
            .into_iter()
            .find(|record| workspace.is_under(&record.workspace))
        else {
            return Ok(());
        };
        let _operation = super::registry::operation(self.path(), &record.workspace_id)?;
        if record.terminal.is_some() || workspace != &record.workspace {
            return Err("the session does not match the owned worktree".to_owned());
        }
        #[cfg(windows)]
        {
            let runtime = runtrol_childproc::process_identity(std::process::id())
                .ok_or("the current Runtime cannot be identified")?;
            if record
                .lifetime
                .is_none_or(|proof| proof.runtime != runtime.into())
            {
                return Err("the structured worktree belongs to an unproven Runtime".to_owned());
            }
        }
        if let Some(bound) = record.session_id.as_deref()
            && bound != session.to_string()
        {
            return Err("the worktree belongs to another session".to_owned());
        }
        if record.state == super::State::Released {
            return Err("the worktree was already released".to_owned());
        }
        if record.session_completed {
            record.session_completed = false;
            super::registry::update(self.path(), record)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(super) struct GenerationProof {
    pub(super) runtime: ProcessStamp,
    pub(super) keeper: Option<ProcessStamp>,
    pub(super) completed: bool,
}

impl GenerationProof {
    pub(super) fn for_owner(
        owner: &runtrol_childproc::Containment,
        runtime: Option<ProcessStamp>,
    ) -> Result<Option<Self>, String> {
        #[cfg(windows)]
        {
            let runtime = runtime.map_or_else(
                || {
                    runtrol_childproc::process_identity(std::process::id())
                        .map(ProcessStamp::from)
                        .ok_or_else(|| "the current Runtime cannot be identified".to_owned())
                },
                Ok,
            )?;
            let identity = owner.keeper_identity();
            if identity.is_some_and(|identity| ProcessStamp::from(identity.runtime()) != runtime) {
                return Err("the worktree keeper belongs to another Runtime".to_owned());
            }
            Ok(Some(Self {
                runtime,
                keeper: identity.map(|identity| identity.keeper().into()),
                completed: false,
            }))
        }
        #[cfg(not(windows))]
        {
            let _owner = (owner, runtime);
            Ok(None)
        }
    }

    pub(super) fn validate(self) -> Result<(), String> {
        self.runtime.validate()?;
        if let Some(keeper) = self.keeper {
            keeper.validate()?;
        }
        if Some(self.runtime) == self.keeper {
            return Err("the worktree keeper is not independent".to_owned());
        }
        if self.completed && self.keeper.is_none() {
            return Err("worktree completion has no keeper proof".to_owned());
        }
        Ok(())
    }

    pub(super) fn belongs_to(self, runtime: ProcessStamp) -> bool {
        self.runtime == runtime
    }

    #[cfg(windows)]
    pub(super) fn complete(&mut self, runtime: ProcessStamp, keeper: ProcessStamp) -> bool {
        if self.runtime != runtime || self.keeper != Some(keeper) {
            return false;
        }
        if self.completed {
            return false;
        }
        self.completed = true;
        true
    }
}

pub(super) fn proven(proof: Option<GenerationProof>, runtime: ProcessStamp) -> bool {
    #[cfg(windows)]
    {
        proof.is_some_and(|proof| proof.runtime == runtime && proof.completed)
    }
    #[cfg(not(windows))]
    {
        let _proof = (proof, runtime);
        true
    }
}

/// Record completion through the existing bounded registry while the exact original home stays pinned.
///
/// # Errors
/// Refuses changed home identity, malformed ownership or a competing row writer. It never removes Git data.
#[cfg(windows)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the callback consumes the non-cloneable OS completion capability after recording it"
)]
pub fn complete_keeper_generation(
    proof: runtrol_childproc::KeeperCompletion,
) -> Result<(), String> {
    let target = proof.target();
    let home = runtrol_provider::AbsPath::canonicalize(
        target
            .directory()
            .to_str()
            .ok_or("the keeper home is not UTF-8")?,
    )
    .map_err(|error| error.to_string())?;
    let mut held = runtrol_security::ProjectRootGuard::acquire(
        &home,
        runtrol_security::ProjectRootIdentity::from_bytes(target.identity()),
    )
    .map_err(|error| error.to_string())?;
    let paths = runtrol_core::home::Layout::resolve(home).map_err(|error| error.to_string())?;
    super::registry::complete(
        paths.isolated_workspaces(),
        proof.owner().runtime().into(),
        proof.owner().keeper().into(),
        || held.validate().map_err(|error| error.to_string()),
    )
}
