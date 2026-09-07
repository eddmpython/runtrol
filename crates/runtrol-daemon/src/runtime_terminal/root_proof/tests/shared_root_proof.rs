use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;

fn due() -> Instant {
    Instant::now()
        .checked_sub(ROOT_REFRESH_AFTER)
        .expect("fixture age")
}

#[test]
fn replacing_a_receiver_keeps_the_refresh_deadline_bound_to_the_completed_proof() {
    let completed_at = due();
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), completed_at, || true);
    let receiver = proof.subscribe();
    let next_completion = Instant::now();
    proof.completed(Ok(RootCheck {
        completed_at: next_completion,
        value: true,
    }));
    assert!(receiver.has_changed().expect("completion was published"));
    let replacement = proof.subscribe();
    assert!(
        !replacement
            .has_changed()
            .expect("the replacement starts current")
    );
    assert_eq!(
        proof.refresh_at(),
        Some(next_completion + ROOT_REFRESH_AFTER)
    );
    assert_eq!(proof.expires_at(), next_completion + ROOT_PROOF_MAX_AGE);
}

#[tokio::test]
async fn eight_refreshes_share_one_os_call_and_its_actual_completion() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let (started, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = tokio::sync::Mutex::new(wait);
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), due(), move || {
        counted.fetch_add(1, Ordering::SeqCst);
        started.send(()).expect("announce the OS check");
        wait.blocking_lock().recv().expect("release the OS check");
        true
    });
    let mut changed = proof.subscribe();
    let views: Vec<_> = (0..8).map(|_| Arc::clone(&proof)).collect();
    for view in &views {
        view.refresh().expect("request shared refresh");
    }
    entered.recv().await.expect("the OS check starts");
    assert_eq!(
        proof.refresh_at(),
        None,
        "the executing check owns the next wake"
    );
    let before_release = Instant::now();
    for view in &views {
        view.refresh().expect("join the running refresh");
    }
    release.send(()).expect("finish the OS check");
    while proof.state.borrow().running.is_some() {
        changed.changed().await.expect("publish completion");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let completed_at = proof.fresh().expect("the proof is fresh");
    assert_eq!(proof.refresh_at(), Some(completed_at + ROOT_REFRESH_AFTER));
    assert!(completed_at >= before_release);
    for view in &views {
        assert_eq!(view.fresh(), Ok(completed_at));
        view.refresh().expect("reuse a recent completion");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the other timers do not start another wave"
    );
}

#[test]
fn a_timeout_preserves_only_the_existing_absolute_proof_lifetime() {
    let completed_at = due();
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(0)), completed_at, || true);
    proof.completed(Err(RootCheckFailure::TimedOut));
    assert_eq!(proof.fresh(), Ok(completed_at));
    assert_eq!(proof.expires_at(), completed_at + ROOT_PROOF_MAX_AGE);
    let expired = Instant::now()
        .checked_sub(ROOT_PROOF_MAX_AGE + Duration::from_millis(1))
        .expect("fixture age");
    proof.set_test_completion(expired);
    proof.completed(Err(RootCheckFailure::TimedOut));
    assert_eq!(proof.fresh(), Err(RootCheckFailure::Stale));
    assert_eq!(proof.expires_at(), expired + ROOT_PROOF_MAX_AGE);
}

#[test]
fn a_shared_denial_immediately_overrides_every_views_recent_proof() {
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), Instant::now(), || true);
    let other_view = Arc::clone(&proof);
    proof.completed(Ok(RootCheck {
        completed_at: Instant::now(),
        value: false,
    }));
    assert_eq!(other_view.fresh(), Err(RootCheckFailure::Denied));
    assert_eq!(other_view.refresh(), Err(RootCheckFailure::Denied));
    proof.completed(Err(RootCheckFailure::TimedOut));
    proof.completed(Ok(RootCheck {
        completed_at: Instant::now(),
        value: true,
    }));
    assert_eq!(
        other_view.fresh(),
        Err(RootCheckFailure::Denied),
        "late outcomes cannot undo an OS denial"
    );
}

#[test]
fn a_worker_failure_cannot_be_revived_by_a_later_timeout_or_success() {
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), Instant::now(), || true);
    proof.completed(Err(RootCheckFailure::WorkerFailed));
    proof.completed(Err(RootCheckFailure::TimedOut));
    proof.completed(Ok(RootCheck {
        completed_at: Instant::now(),
        value: true,
    }));
    assert_eq!(proof.fresh(), Err(RootCheckFailure::WorkerFailed));
}

#[test]
fn applying_a_proof_keeps_its_actual_age_and_rejects_an_old_success() {
    let completed_at = due();
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), completed_at, || true);
    proof.completed(Ok(RootCheck {
        completed_at,
        value: true,
    }));
    assert_eq!(proof.fresh(), Ok(completed_at));
    proof.completed(Ok(RootCheck {
        completed_at: Instant::now()
            .checked_sub(Duration::from_secs(2))
            .expect("fixture age"),
        value: true,
    }));
    assert_eq!(
        proof.fresh(),
        Ok(completed_at),
        "an old success cannot refresh the proof"
    );
}

#[tokio::test]
async fn dropping_the_final_view_cancels_a_queued_check_without_an_owner_cycle() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let permits = Arc::new(Semaphore::new(0));
    let proof = SharedRootProof::new(Arc::clone(&permits), due(), move || {
        counted.fetch_add(1, Ordering::SeqCst);
        true
    });
    let owner = Arc::downgrade(&proof);
    proof.refresh().expect("queue the refresh");
    tokio::task::yield_now().await;
    drop(proof);
    assert!(
        owner.upgrade().is_none(),
        "the task holds only a weak owner"
    );
    permits.add_permits(1);
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a closed view starts no OS call"
    );
}

#[tokio::test]
async fn a_started_os_check_keeps_its_permit_until_it_really_finishes() {
    let permits = Arc::new(Semaphore::new(1));
    let (started, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = tokio::sync::Mutex::new(wait);
    let proof = SharedRootProof::new(Arc::clone(&permits), due(), move || {
        started.send(()).expect("announce started check");
        wait.blocking_lock().recv().expect("finish the check");
        true
    });
    proof.refresh().expect("request refresh");
    entered.recv().await.expect("the check starts");
    drop(proof);
    assert_eq!(permits.available_permits(), 0);
    release.send(()).expect("finish the actual OS call");
    let permit = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
        .await
        .expect("the worker releases its permit")
        .expect("the lane stays open");
    drop(permit);
}

#[tokio::test]
async fn a_timed_out_running_check_remains_single_flight_until_os_completion() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let (started, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = tokio::sync::Mutex::new(wait);
    let proof = SharedRootProof::new(Arc::new(Semaphore::new(2)), due(), move || {
        counted.fetch_add(1, Ordering::SeqCst);
        started.send(()).expect("announce running OS check");
        wait.blocking_lock().recv().expect("finish OS check");
        true
    });
    let completed_at = proof.state.borrow().completed_at;
    let mut changed = proof.subscribe();
    proof.refresh().expect("start a check");
    entered.recv().await.expect("OS check starts");
    while proof.state.borrow().running.is_some() {
        changed
            .changed()
            .await
            .expect("the bounded waiter times out");
    }
    assert_eq!(
        proof.refresh_at(),
        None,
        "a timed-out OS call cannot start a polling loop"
    );
    proof
        .refresh()
        .expect("the running OS check still owns the flight");
    release.send(()).expect("finish the real OS call");
    while proof.busy.load(Ordering::Acquire) {
        changed.changed().await.expect("the real OS call finishes");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(proof.state.borrow().completed_at, completed_at);
}
