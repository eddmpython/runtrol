//! The finite off-reactor owner of managed process birth, publication and claim binding.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use runtrol_childproc::{Program, PtySize};
use runtrol_core::terminal::{Attachment, Terminal, TerminalLaunch};
use runtrol_provider::{AbsPath, ProviderId, TerminalId};

use super::{MAX_HOSTED_TERMINALS, TerminalOpenError, TerminalOperation, WorkerLaunch};
use crate::Composed;
use crate::courier_gate::Minted;
use crate::native_claims::TerminalClaimGuard;

pub(super) struct PreparedLaunch {
    pub(super) terminal_id: TerminalId,
    pub(super) provider: ProviderId,
    pub(super) native: Option<Box<str>>,
    pub(super) cwd: AbsPath,
    pub(super) program: Program,
    pub(super) arguments: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    pub(super) env_unset: Vec<String>,
    pub(super) size: PtySize,
    pub(super) minted: Minted,
    pub(super) reservation: TerminalClaimGuard,
    pub(super) worker: Option<WorkerLaunch>,
}

impl PreparedLaunch {
    pub(super) async fn open(
        self,
        composed: &Arc<Composed>,
        operation: TerminalOperation,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        let composed = Arc::clone(composed);
        let caller = super::operations::LaunchCaller::new();
        let cancelled = Arc::clone(&caller.0);
        let runtime = tokio::runtime::Handle::current();
        // Cancellation before birth refuses the launch. After birth this owner finishes publication,
        // even if its caller disappears, so every process retains observable cleanup ownership.
        let opened = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            runtime.block_on(self.publish(&composed, &cancelled))
        })
        .await
        .map_err(|error| {
            TerminalOpenError::Provider(format!("the terminal launch owner failed: {error}"))
        })?;
        drop(caller);
        opened
    }

    async fn publish(
        self,
        composed: &Arc<Composed>,
        cancelled: &AtomicBool,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        let worker = self.worker.as_ref();
        let terminal = {
            // Keep the established table-to-gate lock order through final admission and process birth.
            let mut terminals = composed.terminals.lock().await;
            let held = terminals
                .len()
                .saturating_add(composed.courier_gate.pending_spawns().await)
                .saturating_sub(usize::from(worker.is_some()));
            if held >= MAX_HOSTED_TERMINALS {
                return Err(TerminalOpenError::NoRoom {
                    held,
                    limit: MAX_HOSTED_TERMINALS,
                });
            }
            let start = || {
                if cancelled.load(Ordering::Acquire) {
                    return Err(TerminalOpenError::Provider(
                        "the terminal open was cancelled before launch".to_owned(),
                    ));
                }
                let terminal = Terminal::open(&TerminalLaunch {
                    program: &self.program,
                    arguments: self.arguments,
                    cwd: &self.cwd,
                    env: self.env,
                    env_unset: self.env_unset,
                    size: self.size,
                })
                .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
                let root = runtrol_childproc::process_identity(terminal.pid());
                Ok::<_, TerminalOpenError>((terminal, root))
            };
            let terminal = if let Some(worker) = worker {
                composed
                    .courier_gate
                    .launch_worker(
                        self.minted,
                        &worker.owned.ticket,
                        || worker.validate(composed),
                        start,
                        |reason| TerminalOpenError::Provider(reason.to_owned()),
                    )
                    .await?
            } else {
                composed.courier_gate.launch(self.minted, start).await?
            };
            let key = self
                .native
                .as_ref()
                .map(|native| (self.provider, native.clone()));
            terminals.insert(
                self.terminal_id,
                self.provider,
                key,
                terminal.clone(),
                self.cwd,
                self.native,
            );
            if let Some(worker) = worker
                && let Some(opened) = terminals.by_id.get_mut(&self.terminal_id)
            {
                opened.spawned = Some(Arc::clone(&worker.owned));
            }
            composed
                .open_terminals
                .store(terminals.len(), Ordering::Release);
            terminal
        };
        // Every registered process has an exit observer before any fallible ownership commit.
        super::forget_on_exit(Arc::clone(composed), self.terminal_id, &terminal);
        if let Err(error) = self.reservation.commit() {
            drop(terminal.kill());
            return Err(error.into());
        }
        if let Some(worker) = worker {
            let binding = match runtrol_childproc::process_identity(terminal.pid()) {
                Some(process) => composed.isolated_workspaces.lock().await.bind_terminal(
                    &worker.owned.ticket,
                    process,
                    &worker.owned.workspace,
                ),
                None => Err("the worker process could not be identified".to_owned()),
            };
            if let Err(error) = binding {
                drop(terminal.kill());
                return Err(TerminalOpenError::Provider(error));
            }
        }
        let attachment = terminal.attach().await;
        Ok((self.terminal_id, terminal, attachment))
    }
}
