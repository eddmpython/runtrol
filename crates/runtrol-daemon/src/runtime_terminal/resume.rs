//! A cold native resume can use only an exact retained worktree beneath its original project authority.

use super::*;
use crate::isolated_workspace::ownership::TerminalOwner;
use crate::terminal_surface::{ResumeLaunch, ResumedTerminal};

pub(super) async fn prepare(
    composed: &Arc<Composed>,
    authority: &AuthorizedIntegration,
    workspace: &AbsPath,
    target: &TerminalOpenTarget,
) -> Result<Option<ResumeLaunch>, TerminalRuntimeFailure> {
    let native = matches!(target, TerminalOpenTarget::Native { .. });
    let owned = Arc::clone(composed);
    let workspace = workspace.clone();
    let authority = Arc::new(authority.clone());
    tokio::task::spawn_blocking(move || {
        let binding = crate::isolated_workspace::read_resume_binding(
            owned.home.paths().isolated_workspaces(),
            &workspace,
        )
        .map_err(|_| root_authority_failure())?;
        let Some(binding) = binding else {
            return Ok(None);
        };
        // Even a directly approved worktree cannot bypass its cleanup owner with a fresh launch.
        if !native {
            return Err(root_authority_failure());
        }
        let runtime = runtrol_childproc::process_identity(std::process::id())
            .ok_or_else(root_authority_failure)?;
        let launch = ResumeLaunch {
            owned: Arc::new(ResumedTerminal {
                binding,
                owner: TerminalOwner {
                    runtime: runtime.into(),
                    terminal: TerminalId::now(),
                },
            }),
            authority,
        };
        launch
            .validate(&owned)
            .map_err(|_| root_authority_failure())?;
        Ok(Some(launch))
    })
    .await
    .map_err(|_| root_authority_failure())?
}

#[cfg(test)]
#[path = "tests/resume.rs"]
mod tests;
