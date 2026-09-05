//! A finite restart sweep preserves unknown owners and removes only ended, unchanged worktrees.

use std::sync::Arc;

use super::{IsolatedWorkspaceController, State, registry};
use crate::Composed;

pub(crate) fn recover_after_restart(composed: &Arc<Composed>) -> impl Future<Output = ()> + use<> {
    // Count before scheduling: a draining generation must not exit ahead of its owned cleanup.
    let operation = crate::terminal_surface::TerminalOperation::begin(composed);
    let composed = Arc::clone(composed);
    let runtime = tokio::runtime::Handle::current();
    async move {
        let result = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            runtime.block_on(async {
                composed
                    .isolated_workspaces
                    .lock()
                    .await
                    .recover_ended(&composed.containment)
                    .await
            })
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => report_cleanup(&error),
            Err(error) => report_cleanup(&format!("worktree recovery task failed: {error}")),
        }
    }
}

impl IsolatedWorkspaceController {
    pub(super) async fn recover_ended(
        &mut self,
        containment: &runtrol_childproc::Containment,
    ) -> Result<(), String> {
        self.records = registry::read(&self.path)?;
        let tickets: Vec<_> = self
            .records
            .iter()
            .filter(|record| record.state != State::Released)
            .filter_map(|record| record.terminal.as_ref())
            .filter(|owner| !owner.ticket.worker.runtime.is_live())
            .map(|owner| owner.ticket)
            .collect();
        for ticket in tickets {
            // A changed directory, live child or busy operation preserves this row without preventing
            // independent ended owners from being reclaimed. No polling or retry task is retained.
            if let Err(error) = self.recover_terminal(containment, &ticket).await {
                report_cleanup(&format!("{}: {error}", ticket.reservation_id()));
            }
        }
        Ok(())
    }
}

#[expect(
    clippy::print_stderr,
    reason = "owned resource cleanup failures use the Runtime operational error stream"
)]
pub(crate) fn report_cleanup(error: &str) {
    eprintln!("runtrol: an ended worker worktree could not be reclaimed: {error}");
}
