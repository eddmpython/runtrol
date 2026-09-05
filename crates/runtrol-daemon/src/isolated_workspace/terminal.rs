//! Terminal worktrees share the existing registry while carrying exact reservation ownership.

use runtrol_childproc::{Containment, resolve};
use runtrol_core::project::ProjectIdentity;
use runtrol_ipc::wire::Response;
use runtrol_provider::{AbsPath, ProcessIdentity};
use serde::{Deserialize, Serialize};

use super::identity::{Directory, VerifiedProject};
use super::ownership::{EndedSpawn, ProcessStamp, SpawnTicket};
use super::{
    GIT_WORKTREE_TIMEOUT, IsolatedWorkspaceController, Operation, Record, State, capture, is_clean,
    registry, release_line, revision,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct TerminalRecord {
    pub(super) ticket: SpawnTicket,
    root_identity: [u8; 24],
    store: Directory,
    pub(super) container: Directory,
    pub(super) directory: Option<Directory>,
    pub(super) process: Option<ProcessStamp>,
    #[serde(default)]
    pub(super) resume: Option<super::resume::ResumeRecord>,
}

impl TerminalRecord {
    pub(super) fn validate(&self, record: &Record) -> Result<(), String> {
        self.ticket.validate()?;
        if self.ticket.reservation_id() != record.workspace_id.as_ref()
            || record.session_id.is_some()
            || record.workspace.parent().as_ref() != Some(&self.container.path)
            || self
                .directory
                .as_ref()
                .is_some_and(|directory| directory.path != record.workspace)
            || (!matches!(record.state, State::Creating | State::Released)
                && self.directory.is_none())
            || (record.state == State::Bound && self.process.is_none())
        {
            return Err("invalid terminal worktree ownership".to_owned());
        }
        if let Some(process) = self.process {
            process.validate()?;
        }
        if let Some(resume) = &self.resume {
            resume.validate()?;
            if record.legacy || !matches!(record.state, State::Bound | State::PreservedDirty) {
                return Err("an unavailable worktree has a resumed occupant".to_owned());
            }
        }
        Ok(())
    }

    pub(super) fn project(&self, record: &Record) -> VerifiedProject {
        VerifiedProject {
            root: Directory {
                path: record.project.clone(),
                identity: self.root_identity,
            },
            store: self.store.clone(),
        }
    }

    pub(super) fn verify(&self, record: &Record, linked: bool) -> Result<(), String> {
        self.project(record).verify()?;
        self.container.verify()?;
        self.directory
            .as_ref()
            .ok_or("the reserved worktree directory identity is unknown")?
            .verify()?;
        if linked {
            let identity =
                ProjectIdentity::discover(record.workspace.clone()).map_err(|e| e.to_string())?;
            if identity.worktree() != &record.workspace
                || identity.common_store() != &self.store.path
            {
                return Err("the terminal worktree changed Git ownership".to_owned());
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct PreparedWorkspace {
    pub(crate) workspace: AbsPath,
    pub(crate) base_commit: Box<str>,
    pub(crate) workspace_identity: [u8; 24],
    pub(crate) container_identity: [u8; 24],
}

impl PreparedWorkspace {
    fn from_record(record: &Record) -> Result<Self, String> {
        let owner = record
            .terminal
            .as_ref()
            .ok_or("the prepared worktree has no owner")?;
        let directory = owner
            .directory
            .as_ref()
            .ok_or("the prepared worktree has no captured directory identity")?;
        Ok(Self {
            workspace: record.workspace.clone(),
            base_commit: record.base_commit.clone(),
            workspace_identity: directory.identity,
            container_identity: owner.container.identity,
        })
    }
}

impl IsolatedWorkspaceController {
    pub(crate) async fn prepare_terminal(
        &mut self,
        containment: &Containment,
        ticket: &SpawnTicket,
        project: &VerifiedProject,
    ) -> Result<PreparedWorkspace, String> {
        ticket.validate()?;
        let id = ticket.reservation_id();
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, &id)?,
        };
        self.refresh_for_write()?;
        project.verify()?;
        if let Some(record) = self
            .records
            .iter()
            .find(|record| record.workspace_id.as_ref() == id)
        {
            let owned = record
                .terminal
                .as_ref()
                .ok_or("the request belongs to a structured session")?;
            if owned.ticket != *ticket || owned.project(record) != *project {
                return Err("the terminal worktree ticket changed ownership".to_owned());
            }
            return self.finish_terminal(ticket, &operation).await;
        }
        self.make_room()?;
        let git = resolve("git").map_err(|e| e.to_string())?;
        let base_commit = revision(&git, project.root(), "HEAD", &operation).await?;
        project.verify()?;
        let container = project
            .root()
            .parent()
            .ok_or("the project has no parent directory")?
            .join(".runtrol-worktrees")
            .map_err(|e| e.to_string())?;
        let workspace = container
            .join(&format!("chat-{id}"))
            .map_err(|e| e.to_string())?;
        if workspace.as_std_path().exists() {
            return Err("the terminal worktree target exists without ownership".to_owned());
        }
        std::fs::create_dir_all(container.as_std_path()).map_err(|e| e.to_string())?;
        let container = Directory::read(container)?;
        container.verify()?;
        let terminal = TerminalRecord {
            ticket: *ticket,
            root_identity: project.root.identity,
            store: project.store.clone(),
            container,
            directory: None,
            process: None,
            resume: None,
        };
        self.records.push(Record {
            workspace_id: id.clone().into(),
            request_id: id.clone().into(),
            project: project.root().clone(),
            workspace,
            base_commit,
            session_id: None,
            state: State::Creating,
            revision: 0,
            terminal: Some(terminal),
            legacy: false,
        });
        self.save(&id)?;
        self.finish_terminal(ticket, &operation).await
    }

    async fn finish_terminal(
        &mut self,
        ticket: &SpawnTicket,
        operation: &Operation<'_>,
    ) -> Result<PreparedWorkspace, String> {
        let mut record = self.terminal_record(ticket)?.clone();
        if record.state == State::Released {
            return Err("the terminal worktree was released".to_owned());
        }
        let owned = record
            .terminal
            .as_ref()
            .ok_or("the terminal worktree owner disappeared")?;
        owned.project(&record).verify()?;
        owned.container.verify()?;
        if record.state != State::Creating {
            owned.verify(&record, true)?;
            if record.state == State::Ready {
                let git = resolve("git").map_err(|e| e.to_string())?;
                if revision(&git, &record.workspace, "HEAD", operation).await? != record.base_commit
                {
                    return Err(
                        "the unused terminal worktree moved from its frozen base".to_owned()
                    );
                }
            }
            return PreparedWorkspace::from_record(&record);
        }
        if owned.directory.is_none() {
            if record.workspace.as_std_path().exists() {
                return Err("the reserved directory has no captured identity".to_owned());
            }
            std::fs::create_dir(record.workspace.as_std_path()).map_err(|e| e.to_string())?;
            let directory = Directory::read(record.workspace.clone())?;
            self.terminal_record_mut(ticket)?
                .terminal
                .as_mut()
                .ok_or("the terminal worktree owner disappeared")?
                .directory = Some(directory);
            self.save(&record.workspace_id)?;
            record = self.terminal_record(ticket)?.clone();
        }
        let owned = record
            .terminal
            .as_ref()
            .ok_or("the terminal worktree owner disappeared")?;
        owned.verify(&record, false)?;
        let git = resolve("git").map_err(|e| e.to_string())?;
        if !record.workspace.as_std_path().join(".git").exists() {
            let output = capture(
                &git,
                &[
                    "-C".to_owned(),
                    record.project.to_string(),
                    "worktree".to_owned(),
                    "add".to_owned(),
                    "--detach".to_owned(),
                    record.workspace.to_string(),
                    record.base_commit.to_string(),
                ],
                GIT_WORKTREE_TIMEOUT,
                operation,
            )
            .await
            .map_err(|e| e.to_string())?;
            if !output.succeeded() || output.truncated {
                return Err("Git refused terminal worktree creation".to_owned());
            }
        }
        owned.verify(&record, true)?;
        if revision(&git, &record.workspace, "HEAD", operation).await? != record.base_commit {
            return Err("the terminal worktree moved from its frozen base".to_owned());
        }
        self.terminal_record_mut(ticket)?.state = State::Ready;
        self.save(&record.workspace_id)?;
        PreparedWorkspace::from_record(self.terminal_record(ticket)?)
    }

    pub(crate) fn bind_terminal(
        &mut self,
        ticket: &SpawnTicket,
        process: ProcessIdentity,
        workspace: &AbsPath,
    ) -> Result<(), String> {
        let id = ticket.reservation_id();
        let _operation = registry::operation(&self.path, &id)?;
        self.refresh_for_write()?;
        let record = self.terminal_record(ticket)?;
        let owned = record
            .terminal
            .as_ref()
            .ok_or("the terminal worktree owner disappeared")?;
        owned.verify(record, true)?;
        let process = ProcessStamp::from(process);
        if &record.workspace != workspace {
            return Err("the terminal opened in another worktree".to_owned());
        }
        if record.state == State::Bound && owned.process == Some(process) {
            return Ok(());
        }
        if record.state != State::Ready || owned.process.is_some() {
            return Err("the terminal worktree is not available for binding".to_owned());
        }
        let record = self.terminal_record_mut(ticket)?;
        record
            .terminal
            .as_mut()
            .ok_or("the terminal worktree owner disappeared")?
            .process = Some(process);
        record.state = State::Bound;
        self.save(&id)
    }

    #[cfg(test)]
    pub(super) async fn release_terminal(
        &mut self,
        containment: &Containment,
        ended: &EndedSpawn,
    ) -> Result<Response, String> {
        self.release_terminal_if_present(containment, ended)
            .await?
            .ok_or_else(|| "the exact terminal worktree owner is unknown".to_owned())
    }

    pub(crate) async fn release_terminal_if_present(
        &mut self,
        containment: &Containment,
        ended: &EndedSpawn,
    ) -> Result<Option<Response>, String> {
        let id = ended.ticket.reservation_id();
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, &id)?,
        };
        self.records = registry::read(&self.path)?;
        if !self
            .records
            .iter()
            .any(|record| record.workspace_id.as_ref() == id)
        {
            return Ok(None);
        }
        self.refresh_for_write()?;
        self.remove_terminal(&ended.ticket, &operation)
            .await
            .map(Some)
    }

    pub(super) async fn recover_terminal(
        &mut self,
        containment: &Containment,
        ticket: &SpawnTicket,
    ) -> Result<Response, String> {
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, &ticket.reservation_id())?,
        };
        self.refresh_for_write()?;
        self.terminal_record(ticket)?;
        if ticket.worker.runtime.is_live() {
            return Err("the owning Runtime is live or cannot be inspected".to_owned());
        }
        self.remove_terminal(ticket, &operation).await
    }

    pub(super) async fn remove_terminal(
        &mut self,
        ticket: &SpawnTicket,
        operation: &Operation<'_>,
    ) -> Result<Response, String> {
        let record = self.terminal_record(ticket)?.clone();
        let owned = record
            .terminal
            .as_ref()
            .ok_or("the terminal worktree owner disappeared")?;
        if owned
            .resume
            .as_ref()
            .is_some_and(super::resume::ResumeRecord::is_live)
        {
            return Err("the resumed worktree occupant is live or cannot be inspected".to_owned());
        }
        if owned.process.is_some_and(ProcessStamp::is_live) {
            return Err("the owning terminal process is live or cannot be inspected".to_owned());
        }
        if record.state == State::Released {
            return Ok(release_line(&record, "alreadyRemoved"));
        }
        owned.project(&record).verify()?;
        owned.container.verify()?;
        if record.state == State::Creating && !record.workspace.as_std_path().exists() {
            return self.terminal_transition(ticket, State::Released, "removed");
        }
        owned.verify(&record, false)?;
        if matches!(record.state, State::Creating | State::PreservedDirty)
            && owned.process.is_none()
            && !record.workspace.as_std_path().join(".git").exists()
        {
            if std::fs::read_dir(record.workspace.as_std_path())
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            {
                return self.terminal_transition(ticket, State::PreservedDirty, "preservedDirty");
            }
            std::fs::remove_dir(record.workspace.as_std_path()).map_err(|e| e.to_string())?;
        } else {
            owned.verify(&record, true)?;
            let git = resolve("git").map_err(|e| e.to_string())?;
            if revision(&git, &record.workspace, "HEAD", operation).await? != record.base_commit
                || !is_clean(&git, &record.workspace, operation).await?
            {
                return self.terminal_transition(ticket, State::PreservedDirty, "preservedDirty");
            }
            owned.verify(&record, true)?;
            let output = capture(
                &git,
                &[
                    "-C".to_owned(),
                    record.project.to_string(),
                    "worktree".to_owned(),
                    "remove".to_owned(),
                    record.workspace.to_string(),
                ],
                GIT_WORKTREE_TIMEOUT,
                operation,
            )
            .await
            .map_err(|e| e.to_string())?;
            if !output.succeeded() || output.truncated {
                return Err("Git refused terminal worktree removal".to_owned());
            }
            owned.project(&record).verify()?;
            owned.container.verify()?;
            if record.workspace.as_std_path().exists() {
                return Err("the terminal worktree remains after Git removal".to_owned());
            }
        }
        self.terminal_transition(ticket, State::Released, "removed")
    }

    fn terminal_transition(
        &mut self,
        ticket: &SpawnTicket,
        state: State,
        outcome: &str,
    ) -> Result<Response, String> {
        let record = self.terminal_record_mut(ticket)?;
        record.state = state;
        if state == State::Released
            && let Some(owned) = &mut record.terminal
        {
            owned.resume = None;
        }
        self.save(&ticket.reservation_id())?;
        Ok(release_line(self.terminal_record(ticket)?, outcome))
    }

    fn terminal_record(&self, ticket: &SpawnTicket) -> Result<&Record, String> {
        self.records
            .iter()
            .find(|r| {
                r.terminal
                    .as_ref()
                    .is_some_and(|owned| owned.ticket == *ticket)
            })
            .ok_or_else(|| "the exact terminal worktree owner is unknown".to_owned())
    }

    fn terminal_record_mut(&mut self, ticket: &SpawnTicket) -> Result<&mut Record, String> {
        self.records
            .iter_mut()
            .find(|r| {
                r.terminal
                    .as_ref()
                    .is_some_and(|owned| owned.ticket == *ticket)
            })
            .ok_or_else(|| "the exact terminal worktree owner is unknown".to_owned())
    }
}
