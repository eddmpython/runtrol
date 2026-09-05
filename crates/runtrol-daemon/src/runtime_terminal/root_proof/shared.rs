//! One in-flight check and one completion-time proof, owned by the views that use this exact root.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::{Semaphore, watch};

use super::{ROOT_PROOF_MAX_AGE, RootCheck, RootCheckFailure, fresh_root_proof, run_root_check};

pub(crate) const ROOT_REFRESH_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

pub(crate) struct SharedRootProof {
    permits: Arc<Semaphore>,
    check: Arc<dyn Fn() -> bool + Send + Sync>,
    state: watch::Sender<ProofState>,
    busy: Arc<AtomicBool>,
    #[cfg(test)]
    calls: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    output_checks: std::sync::atomic::AtomicUsize,
}

pub(crate) struct ProofState {
    completed_at: Instant,
    failure: Option<RootCheckFailure>,
    running: Option<tokio::task::AbortHandle>,
}

impl SharedRootProof {
    pub(crate) fn new(
        permits: Arc<Semaphore>,
        completed_at: Instant,
        check: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Arc<Self> {
        #[cfg(test)]
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let check = {
            let counted = Arc::clone(&calls);
            move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                check()
            }
        };
        Arc::new(Self {
            permits,
            check: Arc::new(check),
            state: watch::channel(ProofState {
                completed_at,
                failure: None,
                running: None,
            })
            .0,
            busy: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            calls,
            #[cfg(test)]
            output_checks: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(crate) fn permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<ProofState> {
        self.state.subscribe()
    }

    pub(crate) fn fresh(&self) -> Result<Instant, RootCheckFailure> {
        let state = self.state.borrow();
        if let Some(failure @ (RootCheckFailure::Denied | RootCheckFailure::WorkerFailed)) =
            state.failure
        {
            return Err(failure);
        }
        fresh_root_proof(state.completed_at)?;
        Ok(state.completed_at)
    }

    /// Wake at the existing absolute proof expiry, even when a refresh is still queued or executing.
    pub(crate) fn expires_at(&self) -> Instant {
        self.state.borrow().completed_at + ROOT_PROOF_MAX_AGE
    }

    pub(crate) async fn ensure_fresh_until(
        self: &Arc<Self>,
        deadline: Instant,
    ) -> Result<(), RootCheckFailure> {
        let mut changed = self.subscribe();
        loop {
            match self.fresh() {
                Ok(completed_at) if completed_at <= deadline => return Ok(()),
                Ok(_) => return Err(RootCheckFailure::TimedOut),
                Err(RootCheckFailure::Stale) => self.refresh()?,
                Err(failure) => return Err(failure),
            }
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), changed.changed())
                .await
                .map_err(|_| RootCheckFailure::TimedOut)?
                .map_err(|_| RootCheckFailure::WorkerFailed)?;
        }
    }

    /// Existing view timers request work. The shared owner neither polls nor starts a permanent task.
    pub(crate) fn refresh(self: &Arc<Self>) -> Result<(), RootCheckFailure> {
        let mut result = Ok(());
        self.state.send_if_modified(|state| {
            if let Some(failure @ (RootCheckFailure::Denied | RootCheckFailure::WorkerFailed)) =
                state.failure
            {
                result = Err(failure);
                return false;
            }
            if state.running.is_some()
                || self.busy.load(Ordering::Acquire)
                || state.completed_at.elapsed() < ROOT_REFRESH_AFTER
            {
                return false;
            }
            let owner = Arc::downgrade(self);
            let permits = Arc::clone(&self.permits);
            let check = Arc::clone(&self.check);
            self.busy.store(true, Ordering::Release);
            let busy = BusyCheck {
                busy: Arc::clone(&self.busy),
                state: self.state.clone(),
            };
            let task = tokio::spawn(async move {
                let observing = owner.clone();
                let result = run_root_check(permits, move || {
                    let _busy = busy;
                    let value = check();
                    if !value && let Some(owner) = observing.upgrade() {
                        owner.completed(Ok(RootCheck {
                            completed_at: Instant::now(),
                            value,
                        }));
                    }
                    value
                })
                .await;
                // No strong owner is held across the await. The final view dropping can cancel this queued task.
                if let Some(owner) = owner.upgrade() {
                    owner.completed(result);
                }
            });
            state.running = Some(task.abort_handle());
            // Starting work provides no new authority. Notify only when a check ends or its closure releases.
            false
        });
        result
    }

    fn completed(&self, result: Result<RootCheck<bool>, RootCheckFailure>) {
        self.state.send_modify(|state| {
            state.running = None;
            if matches!(
                state.failure,
                Some(RootCheckFailure::Denied | RootCheckFailure::WorkerFailed)
            ) {
                return;
            }
            let result = match result {
                Ok(proof) if !proof.value => Err(RootCheckFailure::Denied),
                Ok(proof) => proof.fresh().map(|proof| proof.completed_at),
                Err(failure) => Err(failure),
            };
            match result {
                Ok(completed_at) => {
                    state.completed_at = completed_at;
                    state.failure = None;
                }
                Err(failure) => {
                    // A timeout supplies no newer filesystem evidence. It cannot renew or erase a still-fresh proof.
                    state.failure = Some(failure);
                }
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn test_calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn test_output_check(&self) {
        self.output_checks.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn test_output_checks(&self) -> usize {
        self.output_checks.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn checked_now(&self) -> Result<Instant, RootCheckFailure> {
        let value = (self.check)();
        self.completed(Ok(RootCheck {
            completed_at: Instant::now(),
            value,
        }));
        self.fresh()
    }

    #[cfg(test)]
    pub(crate) fn set_test_completion(&self, completed_at: Instant) {
        self.state.send_if_modified(|state| {
            state.completed_at = completed_at;
            false
        });
    }
}

/// Survives a timed-out waiter until its queued closure is canceled or its executing OS call really ends.
struct BusyCheck {
    busy: Arc<AtomicBool>,
    state: watch::Sender<ProofState>,
}

impl Drop for BusyCheck {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
        self.state.send_modify(|_| {});
    }
}

impl Drop for SharedRootProof {
    fn drop(&mut self) {
        if let Some(task) = self.state.borrow().running.as_ref() {
            task.abort();
        }
    }
}

#[cfg(test)]
#[path = "tests/shared_root_proof.rs"]
mod tests;
