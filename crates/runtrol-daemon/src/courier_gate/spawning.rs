//! Spawn reservations share the existing activation lock with synchronous process birth.

use runtrol_provider::{ProcessIdentity, TerminalId};

use crate::isolated_workspace::ownership::{EndedSpawn, SpawnTicket};

use super::commands::active;
use super::{Admitted, CourierGate, GateState, Minted, Registered, session_of};

pub(crate) const MAX_WORKERS_PER_LEAD: usize = 2;

pub(super) struct Worker {
    ticket: SpawnTicket,
    phase: Phase,
}

enum Phase {
    Pending,
    Live(Option<ProcessIdentity>),
}

impl GateState {
    pub(super) fn register(&mut self, minted: Minted, root: Option<ProcessIdentity>) {
        self.sessions.insert(
            minted.session,
            Registered {
                token: minted.token,
                root,
                activation: 0,
                enabled: false,
                authority: None,
                waits: std::sync::Arc::new(tokio::sync::Semaphore::new(
                    runtrol_courier::wire::SESSION_WAIT_SLOTS,
                )),
            },
        );
    }

    fn pending(&self, ticket: &SpawnTicket) -> Result<(), &'static str> {
        let lead = session_of(ticket.lead.terminal).map_err(|_| "invalid lead identity")?;
        if !active(self, lead, ticket.activation) {
            return Err("the lead's dialogue activation ended");
        }
        if !self
            .workers
            .get(&ticket.worker.terminal)
            .is_some_and(|worker| {
                worker.ticket == *ticket && matches!(worker.phase, Phase::Pending)
            })
        {
            return Err("the exact spawn reservation ended");
        }
        Ok(())
    }
}

impl CourierGate {
    pub(crate) async fn spawn_authority(
        &self,
        admitted: Admitted,
    ) -> Result<std::sync::Arc<crate::runtime_auth::AuthorizedIntegration>, &'static str> {
        let state = self.state.lock().await;
        let activation = admitted.activation.ok_or("dialogue is disabled")?;
        if !active(&state, admitted.session, activation) {
            return Err("the lead's dialogue activation ended");
        }
        state
            .sessions
            .get(&admitted.session)
            .and_then(|registered| registered.authority.clone())
            .ok_or("the dialogue activation has no local integration authority")
    }

    /// Reserve before discovery or Git. A cancelled preparation occupies capacity until its cleanup owns it.
    pub(crate) async fn reserve_spawn(
        &self,
        admitted: Admitted,
        runtime: ProcessIdentity,
        hosted: usize,
        task: Option<runtrol_courier::BoundedUtf8>,
        timeout_ms: u64,
    ) -> Result<(SpawnTicket, Option<runtrol_courier::Receipt>), String> {
        if timeout_ms == 0 || timeout_ms > runtrol_courier::Limits::INITIAL.max_deadline_millis {
            return Err("spawn exceeds the courier deadline ceiling".to_owned());
        }
        let activation = admitted.activation.ok_or("dialogue is disabled")?;
        let lead: TerminalId = admitted
            .session
            .to_string()
            .parse()
            .map_err(|_| "invalid lead")?;
        let mut state = self.state.lock().await;
        if !active(&state, admitted.session, activation) {
            return Err("the lead's dialogue activation ended".to_owned());
        }
        if state.workers.contains_key(&lead) {
            return Err("a worker cannot start another worker".to_owned());
        }
        if state
            .workers
            .values()
            .filter(|worker| worker.ticket.lead.terminal == lead)
            .count()
            >= MAX_WORKERS_PER_LEAD
        {
            return Err("the lead already has its maximum pending or live workers".to_owned());
        }
        let pending = state
            .workers
            .values()
            .filter(|worker| matches!(worker.phase, Phase::Pending))
            .count();
        if hosted.saturating_add(pending) >= crate::terminal_surface::MAX_HOSTED_TERMINALS {
            return Err("the hosted terminal capacity is reserved or occupied".to_owned());
        }
        let ticket = SpawnTicket::new(runtime, lead, TerminalId::now(), activation)?;
        let initial = task
            .map(|body| {
                let now = super::commands::now();
                let target =
                    session_of(ticket.worker.terminal).map_err(|error| error.to_string())?;
                state
                    .courier
                    .reserve_initial(
                        runtrol_courier::CallEnvelope::tell(
                            admitted.session,
                            target,
                            body,
                            now.plus(timeout_ms),
                        ),
                        now,
                    )
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        state.workers.insert(
            ticket.worker.terminal,
            Worker {
                ticket,
                phase: Phase::Pending,
            },
        );
        self.changed.notify_waiters();
        Ok((ticket, initial))
    }

    /// Called while holding the terminal table, preserving the shared table-then-gate admission order.
    pub(crate) async fn pending_spawns(&self) -> usize {
        self.state
            .lock()
            .await
            .workers
            .values()
            .filter(|worker| matches!(worker.phase, Phase::Pending))
            .count()
    }

    /// Final authority and process creation have no cancellation point between them.
    pub(crate) async fn launch_worker<T, E>(
        &self,
        minted: Minted,
        ticket: &SpawnTicket,
        check: impl FnOnce() -> Result<(), E>,
        start: impl FnOnce() -> Result<(T, Option<ProcessIdentity>), E>,
        refused: impl Fn(&'static str) -> E,
    ) -> Result<T, E> {
        let mut state = self.state.lock().await;
        if minted.session
            != session_of(ticket.worker.terminal).map_err(|_| refused("invalid worker identity"))?
        {
            return Err(refused("the minted worker does not match its reservation"));
        }
        state.pending(ticket).map_err(refused)?;
        check()?;
        let (started, root) = start()?;
        if let Some(worker) = state.workers.get_mut(&ticket.worker.terminal) {
            worker.phase = Phase::Live(root);
        }
        state.register(minted, root);
        self.changed.notify_waiters();
        Ok(started)
    }

    /// Preparation has stopped and no process was published. Live workers cannot be retired by cancellation.
    pub(crate) async fn cancel_spawn(&self, ticket: &SpawnTicket) -> Option<EndedSpawn> {
        let mut state = self.state.lock().await;
        if !state
            .workers
            .get(&ticket.worker.terminal)
            .is_some_and(|worker| {
                worker.ticket == *ticket && matches!(worker.phase, Phase::Pending)
            })
        {
            return None;
        }
        state.workers.remove(&ticket.worker.terminal);
        if let Ok(session) = session_of(ticket.worker.terminal) {
            state.courier.session_ended(session);
        }
        self.changed.notify_waiters();
        Some(EndedSpawn::after_gate_retired(*ticket))
    }

    /// Called after the terminal supervisor reports exit. A still-live process keeps its reservation.
    pub(crate) async fn ended_worker(
        &self,
        terminal: TerminalId,
    ) -> Result<Option<EndedSpawn>, &'static str> {
        let mut state = self.state.lock().await;
        let Some(worker) = state.workers.get(&terminal) else {
            return Ok(None);
        };
        if let Phase::Live(Some(process)) = worker.phase
            && runtrol_childproc::matches_process_start(process.pid(), process.started())
        {
            return Err("the worker process is still live or cannot be inspected");
        }
        if matches!(worker.phase, Phase::Pending) {
            return Err("the worker preparation still owns its reservation");
        }
        let ticket = worker.ticket;
        state.workers.remove(&terminal);
        Ok(Some(EndedSpawn::after_gate_retired(ticket)))
    }
}
