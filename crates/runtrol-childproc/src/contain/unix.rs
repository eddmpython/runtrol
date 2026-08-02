//! Containment on Unix: one process group per supervised root.
//!
//! # There is no job object here, and this file does not pretend otherwise
//!
//! Two mechanisms are available and neither is complete on its own.
//!
//! A **process group** lets runtrol signal every child with one call, which covers every shutdown runtrol
//! can see coming: an exit, a caught signal, a panic with a handler. Each child is made a group leader of
//! its own group so that signalling one session cannot touch another.
//!
//! No Unix parent-death signal provides a process-tree guarantee. Linux applies it only to the direct child,
//! which can remove the group leader while leaving descendants behind and make safe restart recovery harder.
//! Both Linux and macOS therefore make the same honest promise: clean shutdown kills the group, and a durable
//! identity lets the next daemon reap a group left by an unclean shutdown.
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
    ///
    /// Returns a `Result` it never fails with, so that both platforms present one signature. The other one
    /// makes three kernel calls and any of them can refuse, and a caller that had to branch on which
    /// platform it was compiled for would be the thing this shape exists to prevent.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fallible signature is the cross-platform contract, not an accident"
    )]
    pub(super) const fn establish() -> Result<Self, SpawnError> {
        Ok(Self { _private: () })
    }

    /// What this platform enforces. Answerable without establishing anything.
    pub(super) const fn platform_strength() -> Strength {
        Strength::CleanShutdownOnly {
            why: "Unix has no job object for an entire descendant tree. a kill -9 of runtrol cannot be \
                  intercepted, so exact process groups are recovered at the next startup",
        }
    }

    /// Put the child in its own process group, and ask for a parent-death signal where that exists.
    #[expect(
        unsafe_code,
        reason = "pre_exec is inherently unsafe: the closure runs between fork and exec, where only \
                  async-signal-safe calls are allowed. the argument that this closure qualifies is at \
                  the block"
    )]
    #[expect(
        clippy::unused_self,
        reason = "the receiver states the precondition (containment exists before a command is prepared) and \
                  keeps both platforms' surfaces identical"
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
                let grouped = libc::setpgid(0, 0);
                if grouped != 0 {
                    return Err(std::io::Error::last_os_error());
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
    #[expect(
        clippy::unused_self,
        reason = "the receiver is the surface both platforms share, and this will read the tracked group \
                  list once the supervisor keeps one"
    )]
    pub(super) fn terminate_all(&self) -> Result<(), SpawnError> {
        Err(SpawnError::Containment {
            doing: "terminating every child group",
            detail: "runtrol does not yet track the child groups on this platform, so it cannot honestly \
                     report that they were stopped. use the session-level stop until the supervisor lands"
                .to_owned(),
        })
    }
}
