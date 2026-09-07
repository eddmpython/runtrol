//! The finite off-reactor owner of managed process birth, publication and claim binding.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use runtrol_childproc::{Program, PtySize};
use runtrol_core::terminal::{Attachment, Terminal, TerminalError, TerminalLaunch};
use runtrol_provider::{AbsPath, ProviderId, TerminalId};

use super::{
    MAX_HOSTED_TERMINALS, ResumeLaunch, TerminalOpenError, TerminalOperation, WorkerLaunch,
};
use crate::Composed;
use crate::courier_gate::Minted;
use crate::native_claims::TerminalClaimGuard;

mod authority;
pub(super) use authority::LaunchAuthority;

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
    pub(super) minted: Option<Minted>,
    pub(super) origin: super::TerminalOrigin,
    pub(super) reservation: TerminalClaimGuard,
    pub(super) worker: Option<WorkerLaunch>,
    pub(super) resumed: Option<ResumeLaunch>,
    pub(super) authority: LaunchAuthority,
}

/// A failed host can still own a live child. Publish that exact terminal and its admission before
/// returning the failure; ordinary errors are proof that no child ownership remains.
pub(super) fn opened(
    result: Result<Terminal, TerminalError>,
) -> Result<(Terminal, Option<TerminalOpenError>), TerminalOpenError> {
    match result {
        Ok(terminal) => Ok((terminal, None)),
        Err(TerminalError::CleanupIncomplete {
            cause,
            cleanup,
            terminal,
        }) => Ok((
            terminal,
            Some(TerminalOpenError::Provider(format!(
                "{cause}; the started process has not been confirmed stopped: {cleanup}"
            ))),
        )),
        Err(error) => Err(TerminalOpenError::Provider(error.to_string())),
    }
}

#[cfg(windows)]
struct Birth {
    terminal: Terminal,
    execution: Option<runtrol_core::terminal::PreparedTerminal>,
    failure: Option<TerminalOpenError>,
}

#[cfg(windows)]
impl Birth {
    fn prepare(launch: &TerminalLaunch<'_>) -> Result<Self, TerminalOpenError> {
        match Terminal::prepare(launch) {
            Ok(prepared) => Ok(Self {
                terminal: prepared.terminal(),
                execution: Some(prepared),
                failure: None,
            }),
            Err(error) => opened(Err(error)).map(|(terminal, failure)| Self {
                terminal,
                execution: None,
                failure,
            }),
        }
    }

    fn execute(&mut self) -> Result<(), TerminalOpenError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if let Some(prepared) = self.execution.take() {
            prepared
                .resume()
                .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        }
        Ok(())
    }
}

impl PreparedLaunch {
    #[cfg(all(windows, test))]
    async fn publish(
        self,
        composed: &Arc<Composed>,
        cancelled: &AtomicBool,
        spawn: impl FnOnce(&TerminalLaunch<'_>) -> Result<Terminal, TerminalError>,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        self.publish_prepared(composed, cancelled, |launch| {
            opened(spawn(launch)).map(|(terminal, failure)| Birth {
                terminal,
                execution: None,
                failure,
            })
        })
        .await
    }

    #[cfg(windows)]
    async fn publish_prepared(
        mut self,
        composed: &Arc<Composed>,
        cancelled: &AtomicBool,
        prepare: impl FnOnce(&TerminalLaunch<'_>) -> Result<Birth, TerminalOpenError>,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        let mut reservation = match &self.resumed {
            Some(resumed) => Some(resumed.reserve(composed)?),
            None => None,
        };
        if let Err(error) = self.reserve_preparation(composed, cancelled).await {
            if let Some(reservation) = reservation {
                reservation.abort().map_err(TerminalOpenError::Provider)?;
            }
            return Err(error);
        }
        // ConPTY creation, process loading and host threads do not hold the table or courier gate.
        // The finite launch owner survives caller cancellation and owns the pending hot capacity.
        let prepared = prepare(&TerminalLaunch {
            containment: Some(&composed.containment),
            program: &self.program,
            arguments: std::mem::take(&mut self.arguments),
            cwd: &self.cwd,
            env: std::mem::take(&mut self.env),
            env_unset: std::mem::take(&mut self.env_unset),
            size: self.size,
        });
        let birth = match prepared {
            Ok(birth) => birth,
            Err(error) => {
                composed
                    .terminals
                    .lock()
                    .await
                    .pending
                    .remove(&self.terminal_id);
                if let Some(reservation) = reservation {
                    reservation.abort().map_err(TerminalOpenError::Provider)?;
                }
                return Err(error);
            }
        };
        let root = runtrol_childproc::process_identity(birth.terminal.pid());
        // Durable occupancy is recorded before execution, also outside the shared input locks.
        let binding = match reservation.as_mut() {
            Some(reservation) => reservation.bind(root),
            None => bind_worker(composed, self.worker.as_ref(), root),
        };
        drop(reservation);
        self.publish_birth(composed, cancelled, birth, root, binding)
            .await
    }

    #[cfg(windows)]
    async fn reserve_preparation(
        &self,
        composed: &Composed,
        cancelled: &AtomicBool,
    ) -> Result<(), TerminalOpenError> {
        self.require_current(composed, cancelled)?;
        let mut terminals = composed.terminals.lock().await;
        require_capacity(composed, terminals.occupied(), self.worker.is_some()).await?;
        if self.worker.is_none() {
            terminals.pending.insert(self.terminal_id);
        }
        Ok(())
    }

    #[cfg(windows)]
    fn require_current(
        &self,
        composed: &Composed,
        cancelled: &AtomicBool,
    ) -> Result<(), TerminalOpenError> {
        if cancelled.load(Ordering::Acquire) || composed.draining.load(Ordering::Acquire) {
            return Err(TerminalOpenError::Provider(
                "the terminal open was cancelled or its Runtime is draining".to_owned(),
            ));
        }
        if let Some(resumed) = &self.resumed {
            resumed.validate(composed)?;
        }
        if let Some(worker) = &self.worker {
            worker.validate(composed)?;
        }
        self.authority.validate(composed, &self.cwd)?;
        if matches!(self.authority, LaunchAuthority::Worktree)
            && self.worker.is_none()
            && self.resumed.is_none()
        {
            return Err(TerminalOpenError::Provider(
                "the terminal open has no worktree authority ticket".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(windows)]
    async fn publish_birth(
        mut self,
        composed: &Arc<Composed>,
        cancelled: &AtomicBool,
        mut birth: Birth,
        root: Option<runtrol_provider::ProcessIdentity>,
        binding: Result<(), String>,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        let terminal = birth.terminal.clone();
        let failure = {
            let mut terminals = composed.terminals.lock().await;
            // Only current authority and the one ResumeThread call run under the admission lock.
            let admitted = composed
                .courier_gate
                .publish_prepared(
                    self.minted.take(),
                    self.worker.as_ref().map(|worker| &worker.owned.ticket),
                    root,
                    terminal.process_scope(),
                    || {
                        binding.map_err(TerminalOpenError::Provider)?;
                        self.require_current(composed, cancelled)?;
                        birth.execute()
                    },
                    |reason| TerminalOpenError::Provider(reason.to_owned()),
                )
                .await;
            terminals.pending.remove(&self.terminal_id);
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
            if let Some(opened) = terminals.by_id.get_mut(&self.terminal_id) {
                opened.origin = self.origin;
                opened.spawned = self.worker.as_ref().map(|worker| Arc::clone(&worker.owned));
                opened.resumed = self
                    .resumed
                    .as_ref()
                    .map(|resumed| Arc::clone(&resumed.owned));
            }
            composed
                .open_terminals
                .store(terminals.len(), Ordering::Release);
            admitted.err()
        };
        // Dropping an unactivated preparation requests stop outside the shared input locks.
        drop(birth);
        finish_born(
            composed,
            self.terminal_id,
            terminal,
            self.reservation,
            None,
            Ok(()),
            failure,
        )
        .await
    }

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
            #[cfg(windows)]
            {
                runtime.block_on(self.publish_prepared(&composed, &cancelled, Birth::prepare))
            }
            #[cfg(not(windows))]
            runtime.block_on(self.publish(&composed, &cancelled, Terminal::open))
        })
        .await
        .map_err(|error| {
            TerminalOpenError::Provider(format!("the terminal launch owner failed: {error}"))
        })?;
        drop(caller);
        opened
    }

    #[cfg(not(windows))]
    async fn publish(
        self,
        composed: &Arc<Composed>,
        cancelled: &AtomicBool,
        spawn: impl FnOnce(&TerminalLaunch<'_>) -> Result<Terminal, TerminalError>,
    ) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
        let worker = self.worker.as_ref();
        let resumed = self.resumed.as_ref();
        // Reserve without holding the terminal table. The stripe lease, not the controller mutex,
        // remains with this finite launch owner until the born process has been durably recorded.
        let mut resume_reservation = match resumed {
            Some(resumed) => Some(resumed.reserve(composed)?),
            None => None,
        };
        let mut born_root = None;
        let mut birth_failure = None;
        let opened = async {
            // Keep the established table-to-gate lock order through final admission and process birth.
            let mut terminals = composed.terminals.lock().await;
            require_capacity(composed, terminals.occupied(), worker.is_some()).await?;
            let start = || {
                self.authority.validate(composed, &self.cwd)?;
                if cancelled.load(Ordering::Acquire) {
                    return Err(TerminalOpenError::Provider(
                        "the terminal open was cancelled before launch".to_owned(),
                    ));
                }
                if let Some(resumed) = resumed {
                    resumed.validate(composed)?;
                }
                let (terminal, failure) = opened(spawn(&TerminalLaunch {
                    containment: Some(&composed.containment),
                    program: &self.program,
                    arguments: self.arguments,
                    cwd: &self.cwd,
                    env: self.env,
                    env_unset: self.env_unset,
                    size: self.size,
                }))?;
                birth_failure = failure;
                let root = runtrol_childproc::process_identity(terminal.pid());
                born_root = root;
                Ok::<_, TerminalOpenError>((terminal, root))
            };
            let terminal = if let Some(worker) = worker {
                composed
                    .courier_gate
                    .launch_worker(
                        self.minted.ok_or_else(|| {
                            TerminalOpenError::Provider(
                                "the worker has no courier admission".to_owned(),
                            )
                        })?,
                        &worker.owned.ticket,
                        || worker.validate(composed),
                        start,
                        |reason| TerminalOpenError::Provider(reason.to_owned()),
                    )
                    .await?
            } else {
                match self.minted {
                    Some(minted) => composed.courier_gate.launch(minted, start).await?,
                    None => start()?.0,
                }
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
            if let Some(opened) = terminals.by_id.get_mut(&self.terminal_id) {
                opened.origin = self.origin;
                opened.spawned = worker.map(|launch| Arc::clone(&launch.owned));
                opened.resumed = resumed.map(|launch| Arc::clone(&launch.owned));
            }
            composed
                .open_terminals
                .store(terminals.len(), Ordering::Release);
            Ok::<_, TerminalOpenError>(terminal)
        }
        .await;
        let terminal = match opened {
            Ok(terminal) => terminal,
            Err(error) => {
                if let Some(reservation) = resume_reservation {
                    reservation.abort().map_err(|cleanup| {
                        TerminalOpenError::Provider(format!("{error}; {cleanup}"))
                    })?;
                }
                return Err(error);
            }
        };
        // File transactions never wait while the shared terminal table or gate is held. The native
        // pending claim and workspace stripe protect birth until durable process binding settles.
        let resume_binding = match resume_reservation.as_mut() {
            Some(reservation) => reservation.bind(born_root),
            None => Ok(()),
        };
        drop(resume_reservation);
        finish_born(
            composed,
            self.terminal_id,
            terminal,
            self.reservation,
            worker,
            resume_binding,
            birth_failure,
        )
        .await
    }
}

async fn require_capacity(
    composed: &Composed,
    hosted: usize,
    reserved_worker: bool,
) -> Result<(), TerminalOpenError> {
    let held = hosted
        .saturating_add(composed.courier_gate.pending_spawns().await)
        .saturating_sub(usize::from(reserved_worker));
    if held >= MAX_HOSTED_TERMINALS {
        return Err(TerminalOpenError::NoRoom {
            held,
            limit: MAX_HOSTED_TERMINALS,
        });
    }
    Ok(())
}

async fn finish_born(
    composed: &Arc<Composed>,
    terminal_id: TerminalId,
    terminal: Terminal,
    claim: TerminalClaimGuard,
    worker: Option<&WorkerLaunch>,
    resume_binding: Result<(), String>,
    birth_failure: Option<TerminalOpenError>,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    // Every registered process has an exit observer before any fallible ownership commit.
    super::forget_on_exit(Arc::clone(composed), terminal_id, &terminal);
    if let Err(error) = claim.commit_born() {
        drop(terminal.kill());
        return Err(error.into());
    }
    if let Err(error) = resume_binding {
        drop(terminal.kill());
        return Err(TerminalOpenError::Provider(error));
    }
    if let Err(error) = bind_worker(
        composed,
        worker,
        runtrol_childproc::process_identity(terminal.pid()),
    ) {
        drop(terminal.kill());
        return Err(TerminalOpenError::Provider(error));
    }
    if let Some(error) = birth_failure {
        drop(terminal.kill());
        return Err(error);
    }
    let attachment = terminal.attach().await;
    Ok((terminal_id, terminal, attachment))
}

fn bind_worker(
    composed: &Composed,
    worker: Option<&WorkerLaunch>,
    root: Option<runtrol_provider::ProcessIdentity>,
) -> Result<(), String> {
    let Some(worker) = worker else {
        return Ok(());
    };
    let process = root.ok_or("the worker process could not be identified")?;
    composed.isolated_workspaces.bind_terminal(
        &worker.owned.ticket,
        process,
        &worker.owned.binding.workspace,
    )
}

#[cfg(test)]
#[path = "../tests/launch.rs"]
mod tests;
