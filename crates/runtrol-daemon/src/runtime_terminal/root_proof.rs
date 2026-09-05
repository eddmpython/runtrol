//! Completion-time bounds for filesystem authority checked away from the Runtime reactor.

use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const ROOT_CHECK_SLOTS: usize = 2;
pub(crate) const ROOT_CHECK_DEADLINE: Duration = Duration::from_millis(400);
const ROOT_PROOF_MAX_AGE: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RootCheck<T> {
    pub(crate) completed_at: Instant,
    pub(crate) value: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootCheckFailure {
    TimedOut,
    WorkerFailed,
    Denied,
    Stale,
}

impl RootCheckFailure {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::TimedOut => "timeout",
            Self::WorkerFailed => "worker",
            Self::Denied => "denied",
            Self::Stale => "stale",
        }
    }
}

pub(crate) fn fresh_root_proof(completed_at: Instant) -> Result<(), RootCheckFailure> {
    if completed_at.elapsed() <= ROOT_PROOF_MAX_AGE {
        Ok(())
    } else {
        Err(RootCheckFailure::Stale)
    }
}

impl<T> RootCheck<T> {
    pub(crate) fn fresh(self) -> Result<Self, RootCheckFailure> {
        fresh_root_proof(self.completed_at)?;
        Ok(self)
    }
}

/// Scheduling may delay observation of an already completed check. Preserve its actual completion time so a
/// caller can distinguish an on-time operation from a result too old to authorize an action now.
pub(crate) async fn run_root_check<T, F>(
    permits: Arc<tokio::sync::Semaphore>,
    check: F,
) -> Result<RootCheck<T>, RootCheckFailure>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let deadline = Instant::now() + ROOT_CHECK_DEADLINE;
    let permit = tokio::select! {
        biased;
        permit = permits.acquire_owned() => permit.map_err(|_| RootCheckFailure::WorkerFailed)?,
        () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            return Err(RootCheckFailure::TimedOut);
        }
    };
    if Instant::now() > deadline {
        return Err(RootCheckFailure::TimedOut);
    }
    let mut worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let value = check();
        RootCheck {
            completed_at: Instant::now(),
            value,
        }
    });
    let deadline_wait = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(deadline_wait);
    tokio::select! {
        biased;
        joined = &mut worker => match joined {
            Ok(checked) if checked.completed_at <= deadline => Ok(checked),
            Ok(_) => Err(RootCheckFailure::TimedOut),
            Err(_) => Err(RootCheckFailure::WorkerFailed),
        },
        () = &mut deadline_wait => Err(RootCheckFailure::TimedOut),
    }
}

#[cfg(test)]
#[path = "tests/root_proof.rs"]
mod tests;
