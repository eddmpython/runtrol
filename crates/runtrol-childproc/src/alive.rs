//! Whether a process identifier still names a process that is running.
//!
//! # Why this is asked at all
//!
//! Coding CLIs leave records of themselves behind. One writes a small file per running process into its own
//! configuration directory, and that file is written on a status change and never again: measured 2026-08-28,
//! a file left by a process that had ended twenty minutes earlier still said the model was answering, beside
//! the file of the process that had taken the conversation over. A reader that trusts such a record without
//! asking the operating system reports a conversation as running forever.
//!
//! So the record names a process, and this is the question that makes the record true or stale.
//!
//! # What "alive" means here
//!
//! That the identifier currently names a process that has not exited. Two things it deliberately does not
//! claim:
//!
//! - **Not that it is the same process the record meant.** An identifier is reused after a machine has churned
//!   through enough of them, and nothing the kernel offers by identifier alone rules that out. A caller that
//!   cannot tolerate the mistake has to pair this with something else the record carries, such as the process
//!   start time.
//! - **Not that a process this program may not open has ended.** A refusal is about our rights; answering
//!   "gone" there would be a guess dressed as a fact, so a refusal counts as running.

/// Whether `pid` names a process that is running now.
#[must_use]
pub fn alive(pid: u32) -> bool {
    platform::alive(pid)
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };

    /// Standard synchronization access right removed from the generated windows-sys 0.61 constants.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    #[expect(
        unsafe_code,
        reason = "opening a process and asking whether it has signalled are calls with no safe wrapper. the safety argument is at each block"
    )]
    pub(super) fn alive(pid: u32) -> bool {
        // Synchronise rights are what the wait below needs; the query right is the least that identifies the
        // process at all, and asking for both in one call keeps a single refusal to interpret.
        let rights = SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;

        // SAFETY: `OpenProcess` takes an access mask, an inheritance flag and an identifier, and returns a
        // handle or null. It reads nothing of ours, and the null case is checked immediately below.
        let process = unsafe { OpenProcess(rights, 0, pid) };
        if process.is_null() {
            // Either the process is gone or this program may not open it. The two are told apart by the error:
            // a refusal is about our rights and says nothing about whether the process ended.
            return std::io::Error::last_os_error().raw_os_error() == Some(ACCESS_DENIED);
        }

        // A process handle is signalled the moment the process exits and never before, so a zero-length wait
        // is the question with no hole in it: timing out means still running.
        //
        // The exit code is not what to ask, and the difference is not academic. `STILL_ACTIVE` is 259, an
        // ordinary exit code a program is free to return, and while any handle to the exited process is open
        // (the shell that started it, a parent still holding its child) the exit code stays readable. Measured
        // 2026-08-28: a process that had exited with 259 read as running for as long as a handle was held, and
        // as gone the moment it was closed. That is the exact shape of the fault this module exists to prevent.
        //
        // SAFETY: `process` came from the checked `OpenProcess` above and is opened for `SYNCHRONIZE`, which is
        // what this call requires. Zero milliseconds means it cannot block. It owns no memory of ours.
        let waited = unsafe { WaitForSingleObject(process, 0) };

        // SAFETY: `process` is the handle from above and nothing else holds it. Closed before the answer is
        // read, so a process that will not answer does not leak a handle.
        //
        // Its own result is not checked, and that is safe rather than swallowed: closing a handle this function
        // opened moments ago fails only if it was already invalid, which cannot be true here, and a caller
        // asking whether a process is running could do nothing about it either way.
        let _closed = unsafe { CloseHandle(process) };

        waited == WAIT_TIMEOUT
    }

    /// The refusal that means "you may not look", which is not the same as "there is nothing there".
    const ACCESS_DENIED: i32 = 5;
}

#[cfg(unix)]
mod platform {
    pub(super) fn alive(pid: u32) -> bool {
        // Zero is not a process here, it is the caller's own process group, and the existence check below
        // always succeeds for it. A record naming it would otherwise read as permanently running.
        if pid == 0 {
            return false;
        }
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
        if sent != 0 {
            // A refusal is about our rights, not about whether the process ended.
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        }
        // A process that has exited but has not been reaped still answers that check, and one of those is not
        // running by any meaning a caller here has. Linux says so in the process's own status; the platforms
        // without that file keep the older answer, where an unreaped child reads as running until its parent
        // collects it.
        !linux_zombie(pid)
    }

    /// Whether Linux reports this process as exited and not yet collected.
    ///
    /// The state is the third field of `/proc/<pid>/stat`, after the command name, which is parenthesised and
    /// may itself contain spaces and parentheses. Splitting after the last `)` is what makes that field
    /// findable without parsing a name a program chose.
    #[cfg(target_os = "linux")]
    fn linux_zombie(pid: i32) -> bool {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            // The process ended between the check above and this read, or this kernel does not publish it.
            // Neither says the process is a zombie, and the check above already answered the question.
            return false;
        };
        let Some((_, after_name)) = status.rsplit_once(')') else {
            return false;
        };
        after_name.split_whitespace().next() == Some("Z")
    }

    #[cfg(not(target_os = "linux"))]
    fn linux_zombie(_pid: i32) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::alive;

    #[test]
    fn this_process_is_running_and_an_identifier_no_process_holds_is_not() {
        assert!(alive(std::process::id()), "this process is running");
        // Larger than any identifier either platform hands out, so it names no process on a machine this
        // builds for, which is what the caller is asking about.
        assert!(
            !alive(u32::MAX),
            "an identifier this large names no process"
        );
        // Zero is the platforms' own special case, and the Unix existence check succeeds for it: it means the
        // caller's process group there. A roster record naming it is not a running coding CLI.
        assert!(!alive(0), "zero names no coding CLI on either platform");
    }

    #[test]
    fn a_process_that_exited_with_the_still_active_code_is_not_running() {
        // 259 is `STILL_ACTIVE`, and it is also an exit code any program may return. Reading the exit code
        // cannot tell the two apart while a handle to the exited process is held, and this test holds one:
        // the `Child` is kept until after the question is asked. Measured 2026-08-28, this is what reported a
        // conversation as running twenty minutes after its process had gone.
        let mut child = std::process::Command::new(exit_with_259().0)
            .args(exit_with_259().1)
            .spawn()
            .expect("a child that exits immediately");
        let pid = child.id();
        let status = child.wait().expect("the child is waited for");
        assert_eq!(status.code(), Some(259), "the child exits with 259");
        assert!(!alive(pid), "an exited process is not running");
    }

    /// A command that exits with 259 and nothing else, in the shell each platform ships.
    fn exit_with_259() -> (&'static str, Vec<&'static str>) {
        if cfg!(windows) {
            ("cmd", vec!["/c", "exit 259"])
        } else {
            ("sh", vec!["-c", "exit 259"])
        }
    }
}
