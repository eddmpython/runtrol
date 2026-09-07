//! Real pool callbacks prove cancellation and callback-context ownership.

use std::os::windows::io::{AsHandle as _, OwnedHandle};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use super::*;

pub(super) struct Callback {
    entered: Barrier,
    release: AtomicBool,
    finished: AtomicBool,
}

impl Callback {
    pub(super) fn run(&self) {
        self.entered.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.release.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.finished.store(true, Ordering::Release);
    }
}

#[expect(
    unsafe_code,
    reason = "this fixture owns a private event for the exact Windows wait API"
)]
fn event() -> OwnedHandle {
    super::super::owned(
        // SAFETY: no name or inheritance, and successful ownership transfers exactly once.
        unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) },
        "creating a wait fixture",
    )
    .unwrap()
}

#[expect(unsafe_code, reason = "only the retained fixture event is signaled")]
fn signal(event: &OwnedHandle) {
    // SAFETY: this is the fixture's live writable event handle.
    assert_ne!(unsafe { SetEvent(event.as_raw_handle()) }, 0);
}

#[tokio::test]
async fn events_before_and_after_subscription_complete_without_a_timer() {
    let early = event();
    signal(&early);
    signaled(early.as_handle()).await.unwrap();
    let late = event();
    let registration = Registration::new(late.as_handle()).unwrap();
    signal(&late);
    tokio::time::timeout(Duration::from_secs(1), registration.completed())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelled_wait_leaves_the_kernel_object_available_for_a_later_waiter() {
    let event = event();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), signaled(event.as_handle()))
            .await
            .is_err()
    );
    assert!(!is_signaled(event.as_handle()).unwrap());
    let next = Registration::new(event.as_handle()).unwrap();
    signal(&event);
    tokio::time::timeout(Duration::from_secs(1), next.completed())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn cancelling_an_executing_callback_waits_before_releasing_its_context() {
    let event = event();
    let callback = Arc::new(Callback {
        entered: Barrier::new(2),
        release: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });
    let registration = Registration::register(
        event.as_handle(),
        Box::new(Signal {
            callback: Some(Arc::clone(&callback)),
            ..Signal::default()
        }),
    )
    .unwrap();
    signal(&event);
    callback.entered.wait();
    let (ending, ended) = std::sync::mpsc::channel();
    let (starting, started) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            starting.send(()).unwrap();
            drop(registration);
            ending.send(()).unwrap();
        });
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        let premature = ended.recv_timeout(Duration::from_millis(20));
        callback.release.store(true, Ordering::Release);
        ended.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            premature.is_err(),
            "registration released a still-running callback context"
        );
        assert!(callback.finished.load(Ordering::Acquire));
    });
}
