//! A shared registry location with per-call state; Git never holds a Runtime-wide mutex.

use runtrol_childproc::Containment;
use runtrol_ipc::wire::Response;
use runtrol_provider::{AbsPath, ProcessIdentity};

use super::ownership::{EndedSpawn, SpawnTicket, TerminalOwner};
use super::{
    EndedResume, PreparedWorkspace, Record, Records, ResumeReservation, State, VerifiedProject,
    WorktreeBinding, registry,
};

/// Durable owner of linked worktrees. Existing operation stripes bound concurrent mutations,
/// and the registry's short writer transaction compares the exact row revision at each save.
#[derive(Clone)]
pub(crate) struct IsolatedWorkspaceController {
    path: AbsPath,
}

impl IsolatedWorkspaceController {
    pub(super) fn path(&self) -> &AbsPath {
        &self.path
    }
    pub(crate) fn open(path: AbsPath) -> Result<Self, String> {
        registry::read(&path)?;
        Ok(Self { path })
    }

    fn records(&self) -> Records {
        Records {
            path: self.path.clone(),
            records: Vec::new(),
        }
    }

    pub(crate) fn list(&self) -> Response {
        let records = match registry::read(&self.path) {
            Ok(records) => records,
            Err(message) => return Response::Failed(runtrol_ipc::wire::WireError::plain(&message)),
        };
        Response::IsolatedWorkspaces(
            records
                .iter()
                .filter(|record| record.state != State::Released && record.terminal.is_none())
                .map(Record::line)
                .collect(),
        )
    }

    pub(crate) async fn prepare(
        &self,
        containment: &Containment,
        request_id: &str,
        project: &str,
    ) -> Result<Response, String> {
        self.records()
            .prepare(containment, request_id, project)
            .await
    }

    pub(crate) fn bind(
        &self,
        workspace_id: &str,
        session_id: &str,
        workspace: &str,
    ) -> Result<Response, String> {
        self.records().bind(workspace_id, session_id, workspace)
    }

    #[cfg(test)]
    pub(crate) async fn release(
        &self,
        containment: &Containment,
        workspace_id: Option<&str>,
        session_id: Option<&str>,
        workspace: &str,
    ) -> Result<Response, String> {
        self.records()
            .release(containment, workspace_id, session_id, workspace, None)
            .await
    }

    pub(crate) async fn release_retaining(
        &self,
        containment: &Containment,
        workspace_id: Option<&str>,
        session_id: Option<&str>,
        workspace: &str,
        retained: std::sync::Arc<dyn Send + Sync>,
    ) -> Result<Response, String> {
        self.records()
            .release(
                containment,
                workspace_id,
                session_id,
                workspace,
                Some(retained),
            )
            .await
    }

    pub(crate) async fn prepare_terminal(
        &self,
        containment: &Containment,
        ticket: &SpawnTicket,
        project: &VerifiedProject,
    ) -> Result<PreparedWorkspace, String> {
        self.records()
            .prepare_terminal(containment, ticket, project)
            .await
    }

    pub(crate) fn bind_terminal(
        &self,
        ticket: &SpawnTicket,
        process: ProcessIdentity,
        workspace: &AbsPath,
    ) -> Result<(), String> {
        self.records().bind_terminal(ticket, process, workspace)
    }

    pub(crate) async fn release_terminal_if_present(
        &self,
        containment: &Containment,
        ended: &EndedSpawn,
    ) -> Result<Option<Response>, String> {
        self.records()
            .release_terminal_if_present(containment, ended)
            .await
    }

    #[cfg(test)]
    pub(super) async fn release_terminal(
        &self,
        containment: &Containment,
        ended: &EndedSpawn,
    ) -> Result<Response, String> {
        self.release_terminal_if_present(containment, ended)
            .await?
            .ok_or_else(|| "the exact terminal worktree owner is unknown".to_owned())
    }

    pub(super) async fn recover_ended(&self, containment: &Containment) -> Result<(), String> {
        self.records().recover_ended(containment).await
    }

    #[cfg(test)]
    pub(super) async fn recover_terminal(
        &self,
        containment: &Containment,
        ticket: &SpawnTicket,
    ) -> Result<Response, String> {
        self.records().recover_terminal(containment, ticket).await
    }

    #[cfg(test)]
    pub(crate) fn resume_binding(
        &self,
        workspace: &AbsPath,
    ) -> Result<Option<WorktreeBinding>, String> {
        super::read_resume_binding(&self.path, workspace)
    }

    pub(crate) fn reserve_resume(
        &self,
        containment: &Containment,
        binding: &WorktreeBinding,
        owner: TerminalOwner,
        check: impl FnMut(TerminalOwner) -> Result<Option<EndedResume>, String>,
    ) -> Result<ResumeReservation, String> {
        self.records()
            .reserve_resume(containment, binding, owner, check)
    }

    pub(crate) async fn release_resume(
        &self,
        containment: &Containment,
        ended: &EndedResume,
    ) -> Result<(), String> {
        self.records().release_resume(containment, ended).await
    }
}
