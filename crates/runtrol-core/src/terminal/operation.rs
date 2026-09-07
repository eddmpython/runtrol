//! Retiring a reservation removes its capacity and order waiter before its task is polled again.

#![expect(
    clippy::disallowed_types,
    reason = "reservation registration, poll and cancellation must be synchronous; this lock never crosses await"
)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as SyncMutex, PoisonError};
use std::task::{Context, Poll, Waker};

use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use super::{TERMINAL_OPERATION_ADMISSIONS, TerminalError};

type Ordered = Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>>;

pub(super) struct OperationGate {
    slots: Arc<Semaphore>,
    pub(super) order: Arc<Mutex<()>>,
    waiting: SyncMutex<Waiting>,
}

#[derive(Default)]
struct Waiting {
    next: u64,
    reservations: [Option<Pending>; TERMINAL_OPERATION_ADMISSIONS],
}

struct Pending {
    id: u64,
    slot: OwnedSemaphorePermit,
    ordered: Ordered,
    waker: Option<Waker>,
}

pub(super) struct OperationReservation<'a> {
    gate: &'a OperationGate,
    id: u64,
}

pub(super) struct OperationAdmission {
    _slot: OwnedSemaphorePermit,
    _ordered: OwnedMutexGuard<()>,
}

impl Default for OperationGate {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(TERMINAL_OPERATION_ADMISSIONS)),
            order: Arc::new(Mutex::new(())),
            waiting: SyncMutex::new(Waiting::default()),
        }
    }
}

impl OperationGate {
    pub(super) fn reserve(&self) -> Result<OperationReservation<'_>, TerminalError> {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| TerminalError::Busy)?;
        waiting.next = waiting.next.checked_add(1).ok_or(TerminalError::Busy)?;
        let id = waiting.next;
        let vacant = waiting
            .reservations
            .iter_mut()
            .find(|pending| pending.is_none())
            .ok_or(TerminalError::Busy)?;
        *vacant = Some(Pending {
            id,
            slot,
            ordered: Box::pin(Arc::clone(&self.order).lock_owned()),
            waker: None,
        });
        Ok(OperationReservation { gate: self, id })
    }

    #[cfg(test)]
    pub(super) async fn admit(&self) -> Result<OperationAdmission, TerminalError> {
        self.reserve()?.wait().await
    }

    pub(super) fn supersede(&self) {
        let retired = {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut waiting.reservations)
        };
        for Pending {
            slot,
            ordered,
            waker,
            ..
        } in retired.into_iter().flatten()
        {
            // Dropping the order future unlinks its Tokio mutex waiter synchronously. Merely notifying
            // the old task would leave a new holder waiting behind it until that task ran again.
            drop(ordered);
            drop(slot);
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
}

impl OperationReservation<'_> {
    pub(super) async fn wait(self) -> Result<OperationAdmission, TerminalError> {
        self.await
    }
}

impl Future for OperationReservation<'_> {
    type Output = Result<OperationAdmission, TerminalError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut waiting = self
            .gate
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = waiting
            .reservations
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|pending| pending.id == self.id))
        else {
            return Poll::Ready(Err(TerminalError::Superseded));
        };
        let Some(pending) = entry.as_mut() else {
            return Poll::Ready(Err(TerminalError::Superseded));
        };
        let Poll::Ready(ordered) = pending.ordered.as_mut().poll(context) else {
            pending.waker = Some(context.waker().clone());
            return Poll::Pending;
        };
        let Some(pending) = entry.take() else {
            return Poll::Ready(Err(TerminalError::Superseded));
        };
        Poll::Ready(Ok(OperationAdmission {
            _slot: pending.slot,
            _ordered: ordered,
        }))
    }
}

impl Drop for OperationReservation<'_> {
    fn drop(&mut self) {
        let retired = {
            let mut waiting = self
                .gate
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            waiting
                .reservations
                .iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|pending| pending.id == self.id))
                .and_then(Option::take)
        };
        drop(retired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;

    #[tokio::test]
    async fn replacement_reclaims_capacity_and_order_without_polling_the_retired_waiter() {
        for poll_before_retirement in [false, true] {
            let gate = OperationGate::default();
            let current = gate.admit().await.unwrap();
            let mut retired = Box::pin(gate.reserve().unwrap().wait());
            if poll_before_retirement {
                poll_fn(|context| {
                    assert!(retired.as_mut().poll(context).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            assert!(matches!(gate.reserve(), Err(TerminalError::Busy)));
            gate.supersede();
            let mut next = Box::pin(
                gate.reserve()
                    .expect("capacity returns before the retired task runs again")
                    .wait(),
            );
            poll_fn(|context| {
                assert!(
                    next.as_mut().poll(context).is_pending(),
                    "the current operation retains its order"
                );
                Poll::Ready(())
            })
            .await;
            assert!(matches!(gate.reserve(), Err(TerminalError::Busy)));
            drop(current);
            let admitted = tokio::time::timeout(std::time::Duration::from_millis(50), next)
                .await
                .expect("the retired mutex waiter no longer blocks its replacement")
                .expect("the replacement is current");
            // The old future has deliberately not run since supersede, including while next advances.
            assert!(matches!(retired.await, Err(TerminalError::Superseded)));
            assert_eq!(
                gate.slots.available_permits(),
                TERMINAL_OPERATION_ADMISSIONS - 1
            );
            gate.supersede();
            assert_eq!(
                gate.slots.available_permits(),
                TERMINAL_OPERATION_ADMISSIONS - 1,
                "a current guard is never retired as a waiter"
            );
            drop(admitted);
            assert_eq!(
                gate.slots.available_permits(),
                TERMINAL_OPERATION_ADMISSIONS
            );
            let abandoned = gate.reserve().unwrap();
            drop(abandoned);
            assert_eq!(
                gate.slots.available_permits(),
                TERMINAL_OPERATION_ADMISSIONS
            );
        }
    }
}
