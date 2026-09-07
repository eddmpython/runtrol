//! Filesystem roster admission belongs to the blocking worker, including after caller cancellation.

use std::sync::Arc;

use runtrol_provider::{ProviderError, ProviderId};
use tokio::sync::Semaphore;

#[derive(Debug)]
pub(crate) struct RosterScan {
    slots: Arc<Semaphore>,
}

impl Default for RosterScan {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(1)),
        }
    }
}

impl RosterScan {
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        provider: ProviderId,
        doing: &'static str,
        read: impl FnOnce() -> Result<T, ProviderError> + Send + 'static,
    ) -> Result<T, ProviderError> {
        let permit = Arc::clone(&self.slots)
            .acquire_owned()
            .await
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing,
                detail: error.to_string(),
            })?;
        // Filesystem calls cannot be cancelled. Holding this permit in their worker prevents timed-out
        // observations from admitting more blocked scans, even when discovery replaces the driver instance.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            read()
        })
        .await
        .map_err(|error| ProviderError::Protocol {
            provider,
            doing,
            detail: error.to_string(),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_observer_keeps_its_slot_until_the_blocked_worker_finishes() {
        let scan = Arc::new(RosterScan::default());
        let provider = ProviderId::parse("fixture").expect("valid test provider");
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let first_scan = Arc::clone(&scan);
        let first = tokio::spawn(async move {
            first_scan
                .run(provider, "reading a blocked test roster", move || {
                    started.send(()).expect("the test awaits worker startup");
                    blocked.recv().expect("the test releases its worker");
                    Ok(())
                })
                .await
        });
        running.await.expect("the worker started");
        first.abort();
        assert!(
            first
                .await
                .expect_err("the observer was cancelled")
                .is_cancelled()
        );
        assert!(scan.slots.try_acquire().is_err());

        let (next_started, next_running) = tokio::sync::oneshot::channel();
        let next_scan = Arc::clone(&scan);
        let next = tokio::spawn(async move {
            next_scan
                .run(provider, "reading the next test roster", move || {
                    next_started
                        .send(())
                        .expect("the test awaits the next worker");
                    Ok(())
                })
                .await
        });
        let mut next_running = next_running;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut next_running)
                .await
                .is_err()
        );
        release.send(()).expect("the blocked worker is still alive");
        tokio::time::timeout(std::time::Duration::from_secs(2), next_running)
            .await
            .expect("the next worker is admitted after completion")
            .expect("the next worker started");
        next.await
            .expect("the next observer joined")
            .expect("its roster answered");
        assert!(scan.slots.try_acquire().is_ok());
    }
}
