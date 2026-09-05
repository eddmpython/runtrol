use std::sync::Arc;
use std::time::Duration;

use super::inspect_peer;
use tokio::sync::{Semaphore, oneshot};

#[tokio::test]
async fn cancelled_process_inspection_keeps_its_slot_without_stalling_input() {
    let slots = Arc::new(Semaphore::new(1));
    let greeting = Arc::clone(&slots)
        .try_acquire_owned()
        .expect("greeting admitted");
    let (started, running) = oneshot::channel();
    let (finish, finishing) = std::sync::mpsc::channel();
    let inspection = tokio::spawn(inspect_peer(greeting, move || {
        started
            .send(())
            .expect("the caller is waiting for inspection");
        finishing
            .recv_timeout(Duration::from_secs(2))
            .expect("inspection was released");
        Ok(())
    }));
    tokio::time::timeout(Duration::from_secs(1), running)
        .await
        .expect("executor kept running")
        .expect("inspection started");
    // This is the production current-thread executor: its timer must run even while the OS query is held.
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(slots.available_permits(), 0);
    inspection.abort();
    assert!(
        inspection
            .await
            .expect_err("the waiter was cancelled")
            .is_cancelled()
    );
    assert_eq!(
        slots.available_permits(),
        0,
        "cancelled inspection still owns kernel work"
    );
    finish.send(()).expect("the kernel worker still exists");
    let reclaimed = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
        .await
        .expect("finished inspection releases its slot")
        .expect("semaphore stays open");
    drop(reclaimed);
    assert_eq!(slots.available_permits(), 1);
}
