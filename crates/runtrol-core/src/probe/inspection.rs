//! Bounded filesystem discovery away from the terminal and request executor.

use std::sync::{Arc, LazyLock};

use runtrol_childproc::Program;
use runtrol_provider::Manifest;
use tokio::sync::Semaphore;

use super::{BinFacts, ProbeError, locate};

static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub(super) async fn inspect(manifest: &Manifest) -> Result<(Program, BinFacts), ProbeError> {
    let manifest = manifest.clone();
    run(Arc::clone(&SLOTS), move || {
        let program = locate(&manifest)?;
        let bin = BinFacts::of_program(&program)?;
        Ok((program, bin))
    })
    .await
}

async fn run<T: Send + 'static>(
    slots: Arc<Semaphore>,
    inspect: impl FnOnce() -> Result<T, ProbeError> + Send + 'static,
) -> Result<T, ProbeError> {
    let permit = slots
        .acquire_owned()
        .await
        .map_err(|error| ProbeError::Inspection {
            detail: error.to_string(),
        })?;
    // Filesystem work cannot be interrupted once it starts. Its slot belongs to the worker, so cancelling a
    // caller never admits replacement work on top of an inspection the operating system has not finished.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        inspect()
    })
    .await
    .map_err(|error| ProbeError::Inspection {
        detail: error.to_string(),
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_a_stalled_inspection_keeps_its_slot_and_the_executor_live() {
        let slots = Arc::new(Semaphore::new(1));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let first = tokio::spawn(run(Arc::clone(&slots), move || {
            started.send(()).expect("the test waits for the worker");
            blocked
                .recv()
                .expect("the test releases the filesystem stand-in");
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .expect("blocking discovery leaves the single-thread executor responsive")
            .expect("worker started");
        first.abort();
        assert!(
            first
                .await
                .expect_err("the caller is cancelled")
                .is_cancelled()
        );
        assert_eq!(
            slots.available_permits(),
            0,
            "the running worker still owns admission"
        );
        let (second_started, mut second_ready) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(run(Arc::clone(&slots), move || {
            second_started
                .send(())
                .expect("the second observer remains open");
            Ok(())
        }));
        tokio::task::yield_now().await;
        assert!(matches!(
            second_ready.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).expect("the first worker is alive");
        tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .expect("the completed inspection releases capacity")
            .expect("the second caller remains alive")
            .expect("the second inspection succeeds");
        second_ready
            .await
            .expect("only the finished worker admits its replacement");
    }
}
