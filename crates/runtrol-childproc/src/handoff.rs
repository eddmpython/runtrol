//! What a process passes on to the children it starts, and what it must not.
//!
//! # The defect this exists for
//!
//! Measured. A command with no daemon running starts one, prints its answer, and exits. The operator's shell then
//! waited forever, and nothing was wrong and nothing said so.
//!
//! The reason is that on Windows a child inherits every handle its parent has marked inheritable, not only the three
//! it is given. A command run in a pipeline holds the shell's own pipe as its output, that handle is inheritable, and
//! the daemon it starts therefore holds a copy of it. The daemon outlives the command by design, so the shell's pipe
//! never reaches its end, and `runtrol list | anything` never returns. Giving the daemon no streams of its own does
//! not help: the copy is separate from the three that were replaced.
//!
//! Unix has no such problem, and for a reason rather than by luck: a descriptor is not passed across an exec unless
//! it is deliberately left open, the standard library marks everything it opens close-on-exec, and the three the
//! child does get are the ones it was given. So there the same code is already correct and this does nothing.
//!
//! # Why it is a process-wide setting and not an argument to a spawn
//!
//! Because it is a property of this process, not of one child: what is being changed is whether **this** program's
//! own handles may travel, and every child started afterwards is covered by one call. A library reaching for it per
//! spawn would be a library changing something global on behalf of a caller that did not ask, and would still leave
//! the window open for any spawn it did not own. It belongs at the start of a program, which is where the one caller
//! is.

use crate::error::SpawnError;

/// Stop this process's own standard handles from travelling to anything it starts.
///
/// Call once, before starting anything. What a child is given as its input and output is unaffected: this is about
/// the copies it would otherwise receive in addition to those, which it has no way to know about and no reason to
/// hold.
///
/// # Errors
///
/// [`SpawnError::Handoff`] when the platform refuses. Not swallowed: the failure it prevents is a shell that hangs
/// with nothing to show for it, and a program that could not prevent it should say so while somebody is watching.
pub fn keep_handles_to_ourselves() -> Result<(), SpawnError> {
    platform::keep_handles_to_ourselves()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    use crate::error::SpawnError;

    #[expect(
        unsafe_code,
        reason = "reading this process's standard handles and clearing a flag on them are kernel calls with no safe wrapper. the safety argument is at each block"
    )]
    pub(super) fn keep_handles_to_ourselves() -> Result<(), SpawnError> {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `GetStdHandle` takes one of the three constants the platform defines for it and only reads
            // this process's own table. It borrows nothing and allocates nothing, and every failure is a returned
            // value rather than a state this code has to unwind.
            let handle = unsafe { GetStdHandle(which) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                // This program was started with no such stream, which is ordinary: a process with no console has no
                // standard handles to pass on, so there is nothing here to prevent.
                continue;
            }

            // SAFETY: `handle` came from `GetStdHandle` above and was checked against both values the platform uses
            // to mean "there is none", so it names a handle this process owns. Clearing the inherit flag changes
            // nothing about the handle itself: it stays open, it stays usable, and the streams this program reads
            // and writes are unaffected. The only thing that changes is whether a copy travels to a child.
            let told = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
            if told == 0 {
                return Err(SpawnError::Handoff {
                    detail: std::io::Error::last_os_error().to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
mod platform {
    use crate::error::SpawnError;

    /// Nothing to do, for the reason in the module documentation rather than because nobody looked.
    ///
    /// The signature is the other platform's, because a caller writing a branch on the platform to decide whether
    /// this can fail would be the caller carrying a rule that belongs here.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature on both platforms. the other one can fail and this one cannot"
    )]
    pub(super) fn keep_handles_to_ourselves() -> Result<(), SpawnError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_twice_is_the_same_as_asking_once() {
        // It is called at the start of a program, and a program that grows a second entry point should not have to
        // know whether the first one already did it.
        keep_handles_to_ourselves().expect("the platform allows this");
        keep_handles_to_ourselves().expect("and allows it again");
    }

    #[test]
    fn the_streams_this_process_uses_still_work_afterwards() {
        // The flag being cleared is about what travels to a child. If it changed anything about the handle itself,
        // a program that called this would stop being able to answer the person who ran it.
        use std::io::Write as _;

        keep_handles_to_ourselves().expect("the platform allows this");
        let mut out = std::io::stdout();
        write!(out, "").expect("standard output still works");
        out.flush().expect("and still flushes");
    }
}
