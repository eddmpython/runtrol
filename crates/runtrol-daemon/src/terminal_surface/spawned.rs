//! A worker is an ordinary hosted terminal with an immutable Core-owned worktree binding.

use super::{
    AbsPath, Arc, Attachment, Composed, ProviderId, Terminal, TerminalId, TerminalOpenError,
    open_with_arguments,
};
use crate::isolated_workspace::VerifiedProject;
use crate::isolated_workspace::ownership::SpawnTicket;
use crate::runtime_auth::AuthorizedIntegration;

#[derive(Clone, Debug)]
pub(crate) struct SpawnedTerminal {
    pub(crate) ticket: SpawnTicket,
    pub(crate) project: VerifiedProject,
    pub(crate) workspace: AbsPath,
    pub(crate) base_commit: Box<str>,
    pub(crate) workspace_identity: [u8; 24],
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
        let identity = runtrol_security::ProjectRootIdentity::read(&self.owned.workspace)
            .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        if identity.to_bytes() != self.owned.workspace_identity {
            return Err(TerminalOpenError::Provider(
                "the reserved worktree changed filesystem identity".to_owned(),
            ));
        }
        authorize_spawn_project(composed, &self.authority, &self.owned.project)
    }
}

pub(crate) fn authorize_spawn_project(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    project: &VerifiedProject,
) -> Result<(), TerminalOpenError> {
    if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
        return Err(TerminalOpenError::Provider(
            "this Runtime generation is draining".to_owned(),
        ));
    }
    let current = crate::runtime_serve::refresh_current(composed, authority)
        .map_err(|failure| TerminalOpenError::Provider(failure.message.to_owned()))?;
    if !current
        .grant
        .scopes
        .contains(&runtrol_runtime_protocol::AppScope::SessionStart)
    {
        return Err(TerminalOpenError::Provider(
            "the activation's integration cannot start sessions".to_owned(),
        ));
    }
    let roots = crate::runtime_inventory::authorized_roots(&current).map_err(|_| {
        TerminalOpenError::Provider("the approved project root changed identity".to_owned())
    })?;
    project.verify().map_err(TerminalOpenError::Provider)?;
    if !roots.iter().any(|root| project.root().is_under(&root.path)) {
        return Err(TerminalOpenError::Provider(
            "the lead's project is outside its approved roots".to_owned(),
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
        launch.owned.workspace.clone(),
        size.cols,
        size.rows,
        Some(program),
        Some(arguments),
        false,
        Some(launch),
    )
    .await
}
