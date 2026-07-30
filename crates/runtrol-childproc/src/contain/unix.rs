//! Containment on Unix: a process group, plus a parent-death signal where the kernel offers one.
//!
//! # There is no job object here, and this file does not pretend otherwise
//!
//! Two mechanisms are available and neither is complete on its own.
//!
//! A **process group** lets runtrol signal every child with one call, which covers every shutdown runtrol
//! can see coming: an exit, a caught signal, a panic with a handler. Each child is made a group leader of
//! its own group so that signalling one session cannot touch another.
//!
//! A **parent-death signal** covers the case runtrol cannot see coming. A child asks the kernel to signal it
//! when its parent dies, so a `SIGKILL` of the daemon still reaches the child. This exists on Linux and not
//! on macOS.
//!
//! So on Linux the guarantee is complete, and on macOS it is not. What closes the macOS gap is not something
//! this file can do from inside a process being killed: it is noticing the orphans at the next startup,
//! which needs recorded child identities and therefore the storage crate.
//!
//! # Why the per-child work happens between fork and exec
//!
//! Setting the process group has to happen after the child exists and before it becomes the target program,
//! which is exactly the window `pre_exec` gives. That window is also the most constrained code in the crate:
//! the child is a fork of a possibly-threaded parent, so only async-signal-safe calls are allowed. Both
//! calls here are direct system calls with no allocation and no locking.

use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::contain::Strength;
use crate::error::SpawnError;

/// The process group arrangement every runtrol child gets.
#[derive(Debug)]
pub(super) struct Containment {
    /// Nothing to hold. The mechanisms here are per-child and applied at spawn, so the guard carries no
    /// resource; it exists so that both platforms present the same surface and no call site branches.
    _private: (),
}

impl Containment {
    /// Nothing to establish up front on this platform.
    pub(super) const fn establish() -> Result<Self, SpawnError> {
        Ok(Self { _private: () })
    }

    /// What this platform enforces. Answerable without establishing anything.
    pub(super) const fn platform_strength() -> Strength {
        if cfg!(target_os = "linux") {
            Strength::EvenIfKilled
        } else {
            Strength::CleanShutdownOnly {
                why: "this platform has no parent-death signal and no job object, so a kill -9 of runtrol \
                      cannot be intercepted. leftover agents are found at the next startup instead",
            }
        }
    }

    /// Put the child in its own process group, and ask for a parent-death signal where that exists.
    #[expect(
        unsafe_code,
        reason = "pre_exec is inherently unsafe: the closure runs between fork and exec, where only \
                  async-signal-safe calls are allowed. the argument that this closure qualifies is at \
                  the block"
    )]
    pub(super) fn prepare(&self, command: &mut Command) {
        // SAFETY: `pre_exec` runs in the forked child, between `fork` and `exec`, where the only calls
        // permitted are async-signal-safe ones. The closure below makes two direct system calls and nothing
        // else: no allocation, no locking, no reentrant library code, no access to shared state. That is the
        // whole requirement, and it is the reason this closure is kept to two lines of real work.
        unsafe {
            command.pre_exec(|| {
                // The child leads its own group, so signalling one session cannot reach another. Passing
                // zero for both arguments means "this process, its own group".
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "setpgid takes pid_t, and zero is the documented way to name the calling \
                              process. no value is being narrowed"
                )]
                let grouped = libc::setpgid(0, 0);
                if grouped != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                // Linux only. Asks the kernel to send this signal when the parent dies, which is what covers
                // a kill runtrol cannot intercept. Absent elsewhere, and `strength` says so rather than this
                // failing quietly.
                #[cfg(target_os = "linux")]
                {
                    let requested = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    if requested != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }

                Ok(())
            });
        }
    }

    /// Signal every child group.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] always, for now.
    ///
    /// The honest state of this: killing the groups needs the list of group identifiers, which is the
    /// supervisor's bookkeeping and does not exist until the kernel crate does. Returning an error rather
    /// than `Ok(())` is deliberate. `Ok` from a panic button that did nothing is the worst possible answer,
    /// because the operator would be told their agents were stopped when they are still writing files.
    pub(super) fn terminate_all(&self) -> Result<(), SpawnError> {
        Err(SpawnError::Containment {
            doing: "terminating every child group",
            detail: "runtrol does not yet track the child groups on this platform, so it cannot honestly \
                     report that they were stopped. use the session-level stop until the supervisor lands"
                .to_owned(),
        })
    }
}
