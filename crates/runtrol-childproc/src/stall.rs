//! A harness-only stall detector: when the caller's heartbeat stops, print the stalled thread's backtrace.
//!
//! The daemon runs its control plane on one async thread. The heartbeat trace proved that thread wedges for
//! seconds at a time on the Linux CI hosts, and every breadcrumb placed by hand missed the line that holds it
//! (2026-08-27). This module ends the guessing: the thread that arms it is remembered, a watchdog thread asks
//! the caller's `stalled` predicate once a second, and when it answers yes the remembered thread is signalled
//! and the signal handler prints that thread's backtrace to stderr.
//!
//! The handler allocates, which a signal handler must not do in general. This is accepted here because the
//! detector is armed only under an explicit diagnostic switch by a harness, on a thread that is already stuck,
//! where the worst case is a daemon the harness was about to fail anyway. It lives in this crate because the
//! workspace forbids `unsafe` everywhere else, and the three libc calls need it.

/// Remember the calling thread and start a watchdog that prints its backtrace when `stalled` says so.
///
/// On platforms without the signal machinery this does nothing.
#[cfg(not(unix))]
pub fn arm_stall_backtrace(stalled: impl Fn() -> bool + Send + 'static) {
    drop(stalled);
}

/// Remember the calling thread and start a watchdog that prints its backtrace when `stalled` says so.
///
/// The predicate is asked once a second from the watchdog thread. Each stall is reported once: the next
/// report needs the predicate to go false and true again.
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "recording the calling thread, installing a signal handler, and signalling that thread are libc calls with no safe wrapper in std"
)]
pub fn arm_stall_backtrace(stalled: impl Fn() -> bool + Send + 'static) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ARMED_THREAD: AtomicUsize = AtomicUsize::new(0);

    // SAFETY: `pthread_self` has no preconditions, and `signal` installs a plain `extern "C"` handler for a
    // signal nothing else in this process uses. The thread id is stored as `usize`, which is a real cast on
    // both Unix families (an unsigned long on Linux, a pointer on macOS).
    unsafe {
        ARMED_THREAD.store(thread_key(libc::pthread_self()), Ordering::Release);
        libc::signal(
            libc::SIGUSR1,
            print_backtrace as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    std::thread::spawn(move || {
        let mut reported = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now_stalled = stalled();
            if now_stalled && !reported {
                reported = true;
                let thread = ARMED_THREAD.load(Ordering::Acquire);
                // SAFETY: the thread id was recorded by the armed thread itself, which lives as long as the
                // process; SIGUSR1 carries the handler installed above.
                unsafe {
                    libc::pthread_kill(thread_of(thread), libc::SIGUSR1);
                }
            } else if !now_stalled {
                reported = false;
            }
        }
    });
}

/// A thread id as a key: an unsigned long on Linux, so a checked conversion; the width is the same on
/// every target this runs on and a failure would only ever mean a 32-bit host, which gets no watchdog.
#[cfg(target_os = "linux")]
fn thread_key(thread: libc::pthread_t) -> usize {
    usize::try_from(thread).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn thread_of(key: usize) -> libc::pthread_t {
    libc::pthread_t::try_from(key).unwrap_or(0)
}

/// A thread id as a key on the other Unix families, where the type is already address sized.
///
/// The cast stays because this arm covers more than one family and they do not agree: the BSDs declare
/// this as a pointer, where the cast is the conversion, and Apple's declares it as `usize` already, where
/// the same cast is a no-op that clippy refuses. Written without the cast it stops compiling on a BSD;
/// written with it, the refusal is answered where it happens and nowhere else. If Apple's declaration goes
/// back to a pointer, this expectation goes unfulfilled and says so rather than hiding the change.
#[cfg_attr(
    target_vendor = "apple",
    expect(
        clippy::unnecessary_cast,
        reason = "this family's `pthread_t` is already `usize`, and the sibling families' is not"
    )
)]
#[cfg(all(unix, not(target_os = "linux")))]
fn thread_key(thread: libc::pthread_t) -> usize {
    thread as usize
}

#[cfg(all(unix, not(target_os = "linux")))]
fn thread_of(key: usize) -> libc::pthread_t {
    key as libc::pthread_t
}

#[cfg(unix)]
#[expect(
    clippy::print_stderr,
    reason = "the backtrace exists to reach a harness's captured stderr; this handler is installed only under the diagnostic switch"
)]
extern "C" fn print_backtrace(_signal: libc::c_int) {
    eprintln!(
        "runtrol close trace: STALL BACKTRACE\n{}",
        std::backtrace::Backtrace::force_capture()
    );
}
