use super::*;

async fn observed_after_pause(pause: Duration) -> RootCheck<bool> {
    let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(0);
    let check = tokio::spawn(run_root_check(
        Arc::new(tokio::sync::Semaphore::new(1)),
        move || {
            finished_tx.send(()).expect("announce the completed check");
            true
        },
    ));
    loop {
        match finished_rx.try_recv() {
            Ok(()) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => panic!("check worker ended early"),
        }
    }
    std::thread::sleep(pause);
    check
        .await
        .expect("join the check")
        .expect("the worker completed within its deadline")
}

#[tokio::test(flavor = "current_thread")]
async fn an_on_time_check_preserves_its_completion_time_after_delayed_observation() {
    let checked = observed_after_pause(ROOT_CHECK_DEADLINE + Duration::from_millis(100)).await;
    assert!(checked.value);
    assert!(checked.completed_at.elapsed() >= ROOT_CHECK_DEADLINE);
    checked
        .fresh()
        .expect("the check is still inside the proof lifetime");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stale_success_cannot_become_new_authority_when_the_reactor_resumes() {
    let checked = observed_after_pause(ROOT_PROOF_MAX_AGE + Duration::from_millis(100)).await;
    assert!(checked.value, "the operating-system check itself succeeded");
    assert_eq!(checked.fresh(), Err(RootCheckFailure::Stale));
}

#[tokio::test]
async fn a_worker_that_really_finishes_late_is_refused() {
    let checked = run_root_check(Arc::new(tokio::sync::Semaphore::new(1)), || {
        std::thread::sleep(ROOT_CHECK_DEADLINE + Duration::from_millis(100));
        true
    })
    .await;
    assert_eq!(checked, Err(RootCheckFailure::TimedOut));
}

#[tokio::test]
async fn a_saturated_lane_times_out_without_starting_another_worker() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let checked = run_root_check(permits, || panic!("no permit can start this check")).await;
    assert_eq!(checked, Err(RootCheckFailure::TimedOut));
}

#[tokio::test]
async fn a_failed_worker_has_a_distinct_structural_reason() {
    let checked = run_root_check(Arc::new(tokio::sync::Semaphore::new(1)), || {
        panic!("synthetic root-check worker failure");
    })
    .await;
    assert_eq!(checked, Err(RootCheckFailure::WorkerFailed));
    assert_eq!(RootCheckFailure::WorkerFailed.reason(), "worker");
    assert_eq!(RootCheckFailure::TimedOut.reason(), "timeout");
    assert_eq!(RootCheckFailure::Denied.reason(), "denied");
    assert_eq!(RootCheckFailure::Stale.reason(), "stale");
}

#[test]
fn cancellation_and_timeout_abort_a_blocking_check_that_has_not_started() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    for timeout in [false, true] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .expect("the fixture runtime builds");
        runtime.block_on(async {
            let (started, entered) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).expect("announce the occupied worker");
                wait.recv().expect("release the occupied worker");
            });
            entered.await.expect("the sole worker is occupied");
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&calls);
            let checking = tokio::spawn(run_root_check(
                Arc::new(tokio::sync::Semaphore::new(1)),
                move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    true
                },
            ));
            tokio::task::yield_now().await;
            if timeout {
                assert_eq!(
                    checking.await.expect("bounded waiter ends"),
                    Err(RootCheckFailure::TimedOut)
                );
            } else {
                checking.abort();
                assert!(checking.await.expect_err("canceled waiter").is_cancelled());
            }
            release.send(()).expect("release the exact blocker");
            blocker.await.expect("the blocker ends");
            tokio::task::spawn_blocking(|| ())
                .await
                .expect("drain the blocking queue");
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        });
    }
}
