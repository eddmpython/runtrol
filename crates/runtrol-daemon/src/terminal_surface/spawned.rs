//! A worker is an ordinary hosted terminal with an immutable Core-owned worktree binding.

use super::{
    Arc, Attachment, Composed, ProviderId, Terminal, TerminalId, TerminalOpenError,
    open_with_arguments,
};
use crate::isolated_workspace::ownership::SpawnTicket;
use crate::isolated_workspace::{VerifiedProject, WorktreeBinding};
use crate::runtime_auth::AuthorizedIntegration;

#[derive(Clone, Debug)]
pub(crate) struct SpawnedTerminal {
    pub(crate) ticket: SpawnTicket,
    pub(crate) binding: WorktreeBinding,
    pub(crate) initial_message: Option<runtrol_courier::MessageId>,
}

#[derive(Clone)]
pub(crate) struct WorkerLaunch {
    pub(crate) owned: Arc<SpawnedTerminal>,
    pub(crate) authority: Arc<AuthorizedIntegration>,
    pub(crate) cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) deadline: tokio::time::Instant,
}

impl WorkerLaunch {
    pub(super) fn validate(&self, composed: &Composed) -> Result<(), TerminalOpenError> {
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || tokio::time::Instant::now() >= self.deadline
        {
            return Err(TerminalOpenError::Provider(
                "the spawn request was cancelled or expired".to_owned(),
            ));
        }
        self.owned
            .binding
            .verify()
            .map_err(TerminalOpenError::Provider)?;
        authorize_spawn_project(composed, &self.authority, &self.owned.binding.project)
    }
}

pub(crate) fn authorize_spawn_project(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    project: &VerifiedProject,
) -> Result<(), TerminalOpenError> {
    authorize_project(
        composed,
        authority,
        project,
        runtrol_runtime_protocol::AppScope::SessionStart,
    )
}

pub(super) fn authorize_project(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    project: &VerifiedProject,
    scope: runtrol_runtime_protocol::AppScope,
) -> Result<(), TerminalOpenError> {
    if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
        return Err(TerminalOpenError::Provider(
            "this Runtime generation is draining".to_owned(),
        ));
    }
    let current = crate::runtime_serve::refresh_current(composed, authority)
        .map_err(|failure| TerminalOpenError::Provider(failure.message.to_owned()))?;
    if !current.grant.scopes.contains(&scope) {
        return Err(TerminalOpenError::Provider(
            "the integration cannot perform the requested terminal operation".to_owned(),
        ));
    }
    let roots = crate::runtime_inventory::authorized_roots(&current).map_err(|_| {
        TerminalOpenError::Provider("the approved project root changed identity".to_owned())
    })?;
    project.verify().map_err(TerminalOpenError::Provider)?;
    if !roots.iter().any(|root| project.root().is_under(&root.path)) {
        return Err(TerminalOpenError::Provider(
            "the terminal's source project is outside its approved roots".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) async fn open_worker(
    composed: &Arc<Composed>,
    provider: ProviderId,
    launch: &WorkerLaunch,
    program: runtrol_childproc::Program,
    arguments: Vec<String>,
    size: runtrol_childproc::PtySize,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    open_with_arguments(
        composed,
        provider,
        None,
        launch.owned.binding.workspace.clone(),
        size.cols,
        size.rows,
        Some(program),
        Some(arguments),
        false,
        Some(launch),
        None,
    )
    .await
}
