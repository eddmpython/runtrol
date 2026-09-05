//! The original project authorizes a native resume only through its verified Core-owned worktree.

use std::sync::Arc;

use crate::Composed;
use crate::isolated_workspace::ownership::TerminalOwner;
use crate::isolated_workspace::{EndedResume, ResumeReservation, WorktreeBinding};
use crate::runtime_auth::AuthorizedIntegration;

use super::{Attachment, ProviderId, Terminal, TerminalId, TerminalOpenError};

#[derive(Clone, Debug)]
pub(crate) struct ResumedTerminal {
    pub(crate) binding: WorktreeBinding,
    pub(crate) owner: TerminalOwner,
}

#[derive(Clone)]
pub(crate) struct ResumeLaunch {
    pub(crate) owned: Arc<ResumedTerminal>,
    pub(crate) authority: Arc<AuthorizedIntegration>,
}

impl ResumeLaunch {
    pub(crate) fn validate(&self, composed: &Composed) -> Result<(), TerminalOpenError> {
        let current = crate::runtime_serve::refresh_current(composed, &self.authority)
            .map_err(|failure| TerminalOpenError::Provider(failure.message.to_owned()))?;
        if current.grant.key_generation != self.authority.grant.key_generation
            || current.grant.grant_generation != self.authority.grant.grant_generation
        {
            return Err(TerminalOpenError::Provider(
                "the native resume observation belongs to an earlier approval".to_owned(),
            ));
        }
        self.owned
            .binding
            .verify()
            .map_err(TerminalOpenError::Provider)?;
        super::spawned::authorize_project(
            composed,
            &self.authority,
            &self.owned.binding.project,
            runtrol_runtime_protocol::AppScope::SessionResume,
        )
    }

    pub(super) async fn reserve(
        &self,
        composed: &Composed,
    ) -> Result<ResumeReservation, TerminalOpenError> {
        composed
            .isolated_workspaces
            .lock()
            .await
            .reserve_resume(&self.owned.binding, self.owned.owner, |pending| {
                self.validate(composed).map_err(|error| error.to_string())?;
                if pending.runtime != self.owned.owner.runtime {
                    return Ok(None);
                }
                if !composed
                    .native_claims
                    .terminal_absent(pending.terminal)
                    .map_err(|error| error.to_string())?
                {
                    return Ok(None);
                }
                Ok(Some(EndedResume::after_claim_retired(
                    &self.owned.binding,
                    pending,
                )))
            })
            .map_err(TerminalOpenError::Provider)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the native resume binds its observed identity, program, geometry and exact worktree owner"
)]
pub(crate) async fn open_resumed(
    composed: &Arc<Composed>,
    provider: ProviderId,
    native: &str,
    launch: &ResumeLaunch,
    cols: u16,
    rows: u16,
    program: runtrol_childproc::Program,
    holder_known: bool,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    super::open_with_arguments(
        composed,
        provider,
        Some(native),
        launch.owned.binding.workspace.clone(),
        cols,
        rows,
        Some(program),
        None,
        holder_known,
        None,
        Some(launch),
    )
    .await
}
