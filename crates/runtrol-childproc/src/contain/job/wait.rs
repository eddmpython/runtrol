//! Await exact kernel objects on the Windows pool without a timer or one thread per terminal.

use std::os::windows::io::{AsRawHandle as _, BorrowedHandle};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tokio::sync::Notify;
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolWait, CreateThreadpoolWait, PTP_CALLBACK_INSTANCE, PTP_WAIT, SetThreadpoolWait,
    WaitForSingleObject, WaitForThreadpoolWaitCallbacks,
};

use super::{failure, last_error};
use crate::SpawnError;

#[derive(Default)]
struct Signal {
    completed: AtomicBool,
    outcome: AtomicU32,
    changed: Notify,
    #[cfg(test)]
    callback: Option<std::sync::Arc<tests::Callback>>,
}

struct Registration<'a> {
    wait: PTP_WAIT,
    signal: Box<Signal>,
    _process: BorrowedHandle<'a>,
}

impl<'a> Registration<'a> {
    fn new(process: BorrowedHandle<'a>) -> Result<Self, SpawnError> {
        Self::register(process, Box::<Signal>::default())
    }

    #[expect(
        unsafe_code,
        reason = "the pool borrows the boxed signal until the owning registration joins callbacks"
    )]
    fn register(process: BorrowedHandle<'a>, mut signal: Box<Signal>) -> Result<Self, SpawnError> {
        // SAFETY: the box stays at this address through registration and callback completion. The
        // null environment selects the system pool; the callback never blocks or performs I/O.
        let wait = unsafe {
            CreateThreadpoolWait(
                Some(notify),
                core::ptr::from_mut(signal.as_mut()).cast(),
                core::ptr::null(),
            )
        };
        if wait == 0 {
            return Err(last_error("registering a process exit event"));
        }
        let registration = Self {
            wait,
            signal,
            _process: process,
        };
        // SAFETY: both the wait and the borrowed process handle remain live until Drop joins every
        // callback. A null timeout waits for the process itself and never schedules a timer.
        unsafe { SetThreadpoolWait(wait, process.as_raw_handle(), core::ptr::null()) };
        Ok(registration)
    }

    async fn completed(&self) -> Result<(), SpawnError> {
        while !self.signal.completed.load(Ordering::Acquire) {
            // One callback and one consumer. notify_one retains a permit if it precedes this await.
            self.signal.changed.notified().await;
        }
        let outcome = self.signal.outcome.load(Ordering::Relaxed);
        if outcome != WAIT_OBJECT_0 {
            return Err(failure(
                "observing a process exit event",
                format!("unexpected wait result {outcome}"),
            ));
        }
        Ok(())
    }
}

#[expect(
    unsafe_code,
    reason = "the system calls this pointer only while Registration owns its boxed context"
)]
unsafe extern "system" fn notify(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _wait: PTP_WAIT,
    outcome: u32,
) {
    // SAFETY: Registration::new passes precisely this boxed Signal. Drop joins callbacks before the
    // box is freed, and the callback never closes the registration or waits for another callback.
    let signal = unsafe { &*context.cast::<Signal>() };
    #[cfg(test)]
    if let Some(callback) = &signal.callback {
        callback.run();
    }
    signal.outcome.store(outcome, Ordering::Relaxed);
    signal.completed.store(true, Ordering::Release);
    signal.changed.notify_one();
}

impl Drop for Registration<'_> {
    #[expect(
        unsafe_code,
        reason = "cancel and join the owned wait before releasing the callback context"
    )]
    fn drop(&mut self) {
        // SAFETY: this is the only owner and never runs in its callback. First stop future queueing,
        // then cancel queued callbacks and join any callback already running, then close the wait.
        // The process borrow and boxed Signal outlive this whole sequence, including cancellation.
        unsafe {
            SetThreadpoolWait(self.wait, core::ptr::null_mut(), core::ptr::null());
            WaitForThreadpoolWaitCallbacks(self.wait, 1);
            CloseThreadpoolWait(self.wait);
        }
    }
}

pub(crate) async fn signaled(process: BorrowedHandle<'_>) -> Result<(), SpawnError> {
    if is_signaled(process)? {
        return Ok(());
    }
    Registration::new(process)?.completed().await
}

#[expect(
    unsafe_code,
    reason = "a zero-time wait reads the exact retained kernel object's state"
)]
pub(crate) fn is_signaled(process: BorrowedHandle<'_>) -> Result<bool, SpawnError> {
    // SAFETY: the borrowed handle remains owned for this call and has synchronization access.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(last_error("inspecting a retained process exit")),
    }
}

#[cfg(test)]
#[path = "tests/wait.rs"]
mod tests;
