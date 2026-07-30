//! Containment on Windows: a job object with kill-on-close.
//!
//! # Why this process joins the job, rather than each child being added after it starts
//!
//! Adding a child after spawning it leaves a window in which the child can start a grandchild that never
//! joins the job. Closing that window by suspending the child, assigning it, and resuming it needs a handle
//! to its main thread, which the standard library does not hand out.
//!
//! Assigning **this** process instead has no window at all: a process created by a job member joins that
//! member's job automatically, so every descendant is inside before it runs a single instruction. It also
//! means [`Containment::prepare`] has nothing to do, which is the right amount of per-spawn work.
//!
//! # What holding the handle does
//!
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` tells the kernel to terminate everything in the job when the last
//! handle to it closes. This process holds that handle, and a handle closes when its process ends, however
//! it ends. So an exit, a panic, and a `TerminateProcess` from outside all produce the same outcome, which
//! is the guarantee that matters: nothing runtrol started outlives it.

use std::process::Command;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::contain::Strength;
use crate::error::SpawnError;

/// Exit code reported for processes the job terminates.
///
/// Distinct from anything a coding CLI returns on its own, so a reader of an exit code can tell "runtrol
/// stopped this" from "the program decided to stop".
const TERMINATED_BY_RUNTROL: u32 = 0x_C000_0409;

/// The job every runtrol child lives in.
#[derive(Debug)]
pub(super) struct Containment {
    /// The job handle. Closing it terminates every process inside, which is what makes this a guard.
    job: HANDLE,
}

impl Containment {
    /// Create the job, set kill-on-close, and put this process in it.
    #[expect(
        unsafe_code,
        reason = "creating a job object is a kernel call with no safe wrapper. the safety argument is at the block"
    )]
    pub(super) fn establish() -> Result<Self, SpawnError> {
        // SAFETY: `CreateJobObjectW` takes an optional security descriptor and an optional name, and null
        // for both is the documented way to ask for an unnamed job with default security. It returns a
        // handle or null, and the null case is checked immediately below.
        let job = unsafe { CreateJobObjectW(core::ptr::null(), core::ptr::null()) };
        if job.is_null() || job == INVALID_HANDLE_VALUE {
            return Err(SpawnError::Containment {
                doing: "creating a job object",
                detail: last_error(),
            });
        }

        let guard = Self { job };
        guard.set_kill_on_close()?;
        guard.join_this_process()?;
        Ok(guard)
    }

    /// Ask the kernel to terminate the job's contents when the last handle closes.
    #[expect(
        unsafe_code,
        reason = "setting a job limit is a kernel call with no safe wrapper"
    )]
    fn set_kill_on_close(&self) -> Result<(), SpawnError> {
        // Every field of this struct is a limit, and zero means unlimited, so the default value is the
        // correct starting point and only the one flag below is being set. Taking it from `Default` rather
        // than zeroing the memory removes an `unsafe` block instead of arguing for one, which is the better
        // trade every time it is available.
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        // SAFETY: the handle came from `CreateJobObjectW` and has not been closed. The information class
        // and the struct type match, which is what the call requires: `JobObjectExtendedLimitInformation`
        // expects exactly `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`. The length is taken from the type rather
        // than written by hand, so it cannot disagree with the pointer.
        let ok = unsafe {
            SetInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                core::ptr::from_ref(&limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            )
        };
        if ok == 0 {
            return Err(SpawnError::Containment {
                doing: "setting kill-on-close on the job object",
                detail: last_error(),
            });
        }
        Ok(())
    }

    /// Put this process in the job, so every descendant joins automatically.
    #[expect(
        unsafe_code,
        reason = "assigning a process to a job is a kernel call with no safe wrapper"
    )]
    fn join_this_process(&self) -> Result<(), SpawnError> {
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that is always valid for the calling process
        // and needs no closing. `AssignProcessToJobObject` takes a job handle and a process handle, and both
        // are valid here.
        let ok = unsafe { AssignProcessToJobObject(self.job, GetCurrentProcess()) };
        if ok == 0 {
            return Err(SpawnError::Containment {
                doing: "putting this process in the job object",
                detail: format!(
                    "{}. runtrol may already be inside a job that does not allow nesting",
                    last_error()
                ),
            });
        }
        Ok(())
    }

    /// What this platform enforces. Answerable without establishing anything.
    pub(super) const fn platform_strength() -> Strength {
        Strength::EvenIfKilled
    }

    /// Nothing to do: a child of a job member is already in the job before it runs.
    ///
    /// Takes a receiver it does not read, on purpose. The other platform's version needs one, and having both
    /// present the same shape means no call site has to know which platform it is compiled for. Requiring a
    /// receiver also states the precondition: there is no point preparing a command before containment exists.
    #[expect(
        clippy::unused_self,
        reason = "the receiver is the precondition, and it keeps both platforms' surfaces identical"
    )]
    pub(super) const fn prepare(&self, _command: &mut Command) {}

    /// Terminate everything in the job, without closing the job.
    #[expect(
        unsafe_code,
        reason = "terminating a job is a kernel call with no safe wrapper"
    )]
    pub(super) fn terminate_all(&self) -> Result<(), SpawnError> {
        // SAFETY: the handle came from `CreateJobObjectW` and has not been closed. This terminates the job's
        // contents, including this process, which is exactly what a panic button does; the guard stays valid
        // either way, because terminating a job does not close its handle.
        let ok = unsafe { TerminateJobObject(self.job, TERMINATED_BY_RUNTROL) };
        if ok == 0 {
            return Err(SpawnError::Containment {
                doing: "terminating the job object",
                detail: last_error(),
            });
        }
        Ok(())
    }
}

impl Drop for Containment {
    /// Close the handle, which is what kills the job's contents.
    ///
    /// Deliberately destructive. Holding this value is what holds the containment, so releasing it is the
    /// kill. The daemon keeps it for the process lifetime, and a process that ends by any means closes its
    /// handles, which is where the guarantee comes from.
    #[expect(
        unsafe_code,
        reason = "closing a handle is a kernel call, and closing this one is what enforces the containment"
    )]
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateJobObjectW`, has not been closed, and this runs once because
        // `Drop` runs once. A failure here cannot be acted on: the process is going away and the kernel
        // closes the handle regardless, producing the same termination.
        let closed = unsafe { CloseHandle(self.job) };
        if closed == 0 {
            // Not swallowed: reported. It means the kill may not have happened, and an operator debugging a
            // stray agent needs to see this line. Printing rather than returning, because `Drop` has no
            // channel and panicking during a shutdown would replace one problem with a worse one.
            eprintln!(
                "runtrol: could not close the job object handle ({}). \
                 child processes may have survived",
                last_error()
            );
        }
    }
}

/// The last operating system error, as text.
fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}
