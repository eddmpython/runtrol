//! Whether a process identifier still names a process that is running.
//!
//! # Why this is asked at all
//!
//! Coding CLIs leave records of themselves behind. Claude Code writes one small file per running process into
//! its own configuration directory, and that file is written on a status change and never again: measured on
//! 2.1.250, a file left by a process that ended twenty minutes earlier still said `busy`, beside the file of
//! the process that had taken the conversation over. A reader that trusts such a record without asking the
//! operating system reports a conversation as running forever.
//!
//! So the record names a process, and this is the question that makes the record true or stale.
//!
//! # What "alive" means here
//!
//! That the identifier currently names a process that has not exited. Not that it is the same process the
//! record meant: an identifier is reused after a machine has churned through enough of them, and nothing the
//! kernel offers by identifier alone rules that out. Callers that cannot tolerate the mistake pair this with
//! something the record also carries, such as the process start time.

/// Whether `pid` names a process that is running now.
///
/// A process this program is not allowed to open still counts as running: the kernel refused the caller, not
/// the existence of the process, and answering "gone" there would be a guess dressed as a fact.
#[must_use]
pub fn alive(pid: u32) -> bool {
    platform::alive(pid)
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[expect(
        unsafe_code,
        reason = "opening a process and reading its exit code are calls with no safe wrapper. the safety argument is at each block"
    )]
    pub(super) fn alive(pid: u32) -> bool {
        // SAFETY: `OpenProcess` takes an access mask, an inheritance flag and an identifier, and returns a
        // handle or null. It reads nothing of ours, and the null case is checked immediately below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            // Either the process is gone or this program may not open it. The two are told apart by the error:
            // a refusal is about our rights and says nothing about whether the process ended.
            return std::io::Error::last_os_error().raw_os_error() == Some(ACCESS_DENIED);
        }

        let mut code: u32 = 0;
        // SAFETY: `process` came from the checked `OpenProcess` above, and the pointer is to a local `u32` that
        // lives for the whole call and is shared with nothing.
        let asked = unsafe { GetExitCodeProcess(process, &raw mut code) };

        // SAFETY: `process` is the handle from above and nothing else holds it. Closed before the answer is
        // read, so a process that will not answer does not leak a handle.
        //
        // Its own result is not checked, and that is safe rather than swallowed: closing a handle this function
        // opened moments ago fails only if it was already invalid, which cannot be true here, and a caller
        // asking whether a process is running could do nothing about it either way.
        let _closed = unsafe { CloseHandle(process) };

        // A handle stays openable after the process exits, until every handle to it is closed, so the handle
        // alone proves nothing. The exit code is what separates a running process from one being held open.
        // The constant is declared as the status value it is, and the call writes an exit code; one cast
        // here keeps the comparison honest rather than widening the code to something it is not.
        asked != 0 && code == STILL_ACTIVE.cast_unsigned()
    }

    /// The refusal that means "you may not look", which is not the same as "there is nothing there".
    const ACCESS_DENIED: i32 = 5;
}

#[cfg(unix)]
mod platform {
    pub(super) fn alive(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            // An identifier this platform cannot hold is not one of its processes.
            return false;
        };
        #[expect(
            unsafe_code,
            reason = "the existence check for a process identifier is a system call with no safe wrapper"
        )]
        // SAFETY: signal 0 is the documented no-op: `kill` performs its permission and existence checks and
        // delivers nothing. It owns no memory of ours and cannot stop the target.
        let sent = unsafe { libc::kill(pid, 0) };
        if sent == 0 {
            return true;
        }
        // A refusal is about our rights, not about whether the process ended.
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::alive;

    #[test]
    fn this_process_is_running_and_an_identifier_no_process_holds_is_not() {
        assert!(alive(std::process::id()), "this process is running");
        // Reserved by every platform this builds on for something that is not an ordinary process: on Windows
        // it is the idle process, on Unix it is not a valid target for the existence check. Either way it is
        // never a coding CLI, which is what the caller is asking about.
        assert!(
            !alive(u32::MAX),
            "an identifier this large names no process"
        );
    }
}
