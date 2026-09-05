//! A native resume occupies an existing worktree without inheriting its original worker lineage.

use std::sync::Arc;

use runtrol_provider::{AbsPath, ProcessIdentity};
use serde::{Deserialize, Serialize};

use super::identity::VerifiedProject;
use super::ownership::{ProcessStamp, TerminalOwner};
use super::{IsolatedWorkspaceController, Operation, Record, State, registry, report_cleanup};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreeBinding {
    pub(crate) workspace_id: Box<str>,
    pub(crate) project: VerifiedProject,
    pub(crate) workspace: AbsPath,
    pub(crate) workspace_identity: [u8; 24],
    pub(crate) container_identity: [u8; 24],
    pub(crate) base_commit: Box<str>,
}

impl WorktreeBinding {
    fn from_record(record: &Record) -> Result<Self, String> {
        record.require_current_owner()?;
        let owned = record
            .terminal
            .as_ref()
            .ok_or("the worktree has no terminal ownership")?;
        if !matches!(record.state, State::Bound | State::PreservedDirty) || owned.process.is_none()
        {
            return Err("the worktree has no previously hosted terminal".to_owned());
        }
        owned.verify(record, true)?;
        Ok(Self {
            workspace_id: record.workspace_id.clone(),
            project: owned.project(record),
            workspace: record.workspace.clone(),
            workspace_identity: owned
                .directory
                .as_ref()
                .ok_or("the workspace identity is missing")?
                .identity,
            container_identity: owned.container.identity,
            base_commit: record.base_commit.clone(),
        })
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        self.project.verify()?;
        super::identity::Directory {
            path: self
                .workspace
                .parent()
                .ok_or("the workspace container is missing")?,
            identity: self.container_identity,
        }
        .verify()?;
        super::identity::Directory {
            path: self.workspace.clone(),
            identity: self.workspace_identity,
        }
        .verify()?;
        let identity = runtrol_core::project::ProjectIdentity::discover(self.workspace.clone())
            .map_err(|error| error.to_string())?;
        if identity.worktree() != &self.workspace
            || identity.common_store() != &self.project.store.path
        {
            return Err("the terminal worktree changed Git ownership".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ResumeRecord {
    pub(super) owner: TerminalOwner,
    process: Option<ProcessStamp>,
}

impl ResumeRecord {
    pub(super) fn validate(&self) -> Result<(), String> {
        self.owner.runtime.validate()?;
        if let Some(process) = self.process {
            process.validate()?;
        }
        Ok(())
    }

    pub(super) fn is_live(&self) -> bool {
        self.owner.runtime.is_live() || self.process.is_some_and(ProcessStamp::is_live)
    }
}

/// Lives on the blocking launch owner through birth and binding. A dropped pre-birth reservation clears
/// only its exact pending occupant. After birth, the terminal exit observer owns release instead.
pub(crate) struct ResumeReservation {
    path: AbsPath,
    workspace_id: Box<str>,
    owner: TerminalOwner,
    _lease: Arc<std::fs::File>,
    born: bool,
}

impl ResumeReservation {
    pub(crate) fn bind(&mut self, process: Option<ProcessIdentity>) -> Result<(), String> {
        self.born = true;
        let process = process.ok_or("the resumed terminal process could not be identified")?;
        let mut record = self.current()?;
        let resume = record
            .terminal
            .as_mut()
            .and_then(|owned| owned.resume.as_mut())
            .ok_or("the resume reservation disappeared")?;
        if resume.owner != self.owner || resume.process.is_some() {
            return Err("the resume reservation changed before process binding".to_owned());
        }
        resume.process = Some(process.into());
        registry::update(&self.path, record)?;
        Ok(())
    }

    fn current(&self) -> Result<Record, String> {
        registry::read(&self.path)?
            .into_iter()
            .find(|record| record.workspace_id == self.workspace_id)
            .ok_or_else(|| "the resume worktree disappeared".to_owned())
    }

    fn cancel_pending(&self) -> Result<(), String> {
        let mut record = self.current()?;
        let owned = record
            .terminal
            .as_mut()
            .ok_or("the worktree owner disappeared")?;
        if owned
            .resume
            .as_ref()
            .is_some_and(|resume| resume.owner == self.owner && resume.process.is_none())
        {
            owned.resume = None;
            registry::rollback(&self.path, record)?;
        }
        Ok(())
    }

    /// Return cancellation failure to the launch caller. A surviving exact pending row remains
    /// protected and can be reconciled by a later positive current-Runtime claim-absence proof.
    pub(crate) fn abort(mut self) -> Result<(), String> {
        self.born = true;
        self.cancel_pending()
    }
}

impl Drop for ResumeReservation {
    fn drop(&mut self) {
        if !self.born
            && let Err(error) = self.cancel_pending()
        {
            report_cleanup(&error);
        }
    }
}

/// An exact terminal exit, or positive same-Runtime proof that its native claim was retired.
pub(crate) struct EndedResume {
    workspace_id: Box<str>,
    owner: TerminalOwner,
}

impl EndedResume {
    pub(crate) fn after_terminal_exit(binding: &WorktreeBinding, owner: TerminalOwner) -> Self {
        Self {
            workspace_id: binding.workspace_id.clone(),
            owner,
        }
    }

    pub(crate) fn after_claim_retired(binding: &WorktreeBinding, owner: TerminalOwner) -> Self {
        Self {
            workspace_id: binding.workspace_id.clone(),
            owner,
        }
    }
}

/// Inspect an atomic registry snapshot without waiting for unrelated Git operations. This does not
/// reserve or exclude a live owner: a second viewer may still join before final ownership admission.
pub(crate) fn read_resume_binding(
    path: &AbsPath,
    workspace: &AbsPath,
) -> Result<Option<WorktreeBinding>, String> {
    let records = registry::read(path)?;
    let Some(record) = records
        .iter()
        .find(|record| record.terminal.is_some() && &record.workspace == workspace)
    else {
        if records
            .iter()
            .any(|record| record.terminal.is_some() && workspace.is_under(&record.workspace))
        {
            return Err(
                "a Core-owned worktree can only resume from its recorded workspace root".to_owned(),
            );
        }
        return Ok(None);
    };
    WorktreeBinding::from_record(record).map(Some)
}

impl IsolatedWorkspaceController {
    #[cfg(test)]
    pub(crate) fn resume_binding(
        &self,
        workspace: &AbsPath,
    ) -> Result<Option<WorktreeBinding>, String> {
        read_resume_binding(&self.path, workspace)
    }

    pub(crate) fn reserve_resume(
        &mut self,
        binding: &WorktreeBinding,
        owner: TerminalOwner,
        mut check: impl FnMut(TerminalOwner) -> Result<Option<EndedResume>, String>,
    ) -> Result<ResumeReservation, String> {
        let lease = registry::operation(&self.path, &binding.workspace_id)?;
        self.refresh_for_write()?;
        owner.runtime.validate()?;
        if !owner.runtime.is_live() {
            return Err("the resuming Runtime is not live".to_owned());
        }
        let record = self
            .records
            .iter_mut()
            .find(|record| record.workspace_id == binding.workspace_id)
            .ok_or("the resume worktree disappeared")?;
        if WorktreeBinding::from_record(record)? != *binding {
            return Err("the resume worktree binding changed".to_owned());
        }
        let occupancy = record
            .terminal
            .as_mut()
            .ok_or("the worktree owner disappeared")?;
        let original_retired = check(occupancy.ticket.worker)?;
        let owner_ended = |previous: TerminalOwner, proof: &Option<EndedResume>| {
            !previous.runtime.is_live()
                || (previous.runtime == owner.runtime
                    && proof.as_ref().is_some_and(|ended| {
                        ended.workspace_id == binding.workspace_id && ended.owner == previous
                    }))
        };
        let original_ended = owner_ended(occupancy.ticket.worker, &original_retired);
        let resumed_ended = if let Some(resume) = &occupancy.resume {
            let retired = check(resume.owner)?;
            owner_ended(resume.owner, &retired)
                && !resume.process.is_some_and(ProcessStamp::is_live)
        } else {
            true
        };
        if occupancy.process.is_some_and(ProcessStamp::is_live) || !original_ended || !resumed_ended
        {
            return Err("the worktree already has a live or uninspectable occupant".to_owned());
        }
        occupancy.resume = Some(ResumeRecord {
            owner,
            process: None,
        });
        self.save(&binding.workspace_id)?;
        Ok(ResumeReservation {
            path: self.path.clone(),
            workspace_id: binding.workspace_id.clone(),
            owner,
            _lease: lease,
            born: false,
        })
    }

    pub(crate) async fn release_resume(
        &mut self,
        containment: &runtrol_childproc::Containment,
        ended: &EndedResume,
    ) -> Result<(), String> {
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, &ended.workspace_id)?,
        };
        self.refresh_for_write()?;
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.workspace_id == ended.workspace_id)
        else {
            return Ok(());
        };
        let owned = record
            .terminal
            .as_mut()
            .ok_or("the worktree owner disappeared")?;
        let Some(resume) = &owned.resume else {
            return Ok(());
        };
        if resume.owner != ended.owner {
            return Ok(());
        }
        if resume.process.is_some_and(ProcessStamp::is_live) {
            return Err("the resumed terminal is still live or cannot be inspected".to_owned());
        }
        let ticket = owned.ticket;
        owned.resume = None;
        self.save(&ended.workspace_id)?;
        self.remove_terminal(&ticket, &operation).await?;
        Ok(())
    }
}

/// Bare argv cannot acquire a terminal worktree's native-resume ownership. Legacy structured
/// worktrees keep their existing session binding; this guard protects the terminal-owned rows.
pub(crate) async fn refuse_unbound_worktree(
    composed: &Arc<crate::Composed>,
    workspace: &AbsPath,
) -> Result<(), String> {
    let path = composed.home.paths().isolated_workspaces().clone();
    let workspace = workspace.clone();
    tokio::task::spawn_blocking(move || {
        let records = registry::read(&path)?;
        if records
            .iter()
            .any(|record| record.terminal.is_some() && workspace.is_under(&record.workspace))
        {
            return Err(
                "a Core-owned worktree requires its authenticated native resume".to_owned(),
            );
        }
        Ok(())
    })
    .await
    .map_err(|_| "the worktree ownership check could not complete".to_owned())?
}
