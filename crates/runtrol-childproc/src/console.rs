//! The console window of the current process, when Windows created one only for runtrol.
//!
//! One executable serves both the command line and the desktop window. Marking that executable as a Windows
//! GUI subsystem program would remove its command-line output too, while leaving it as a console subsystem
//! program makes Windows flash a black terminal whenever the desktop personality is launched directly.
//!
//! The safe distinction is ownership. A console with another attached process belongs to the terminal that
//! launched runtrol and must stay visible. A console with only runtrol attached was created solely because a
//! console-subsystem executable started without a terminal, so the desktop personality hides that window.

use crate::error::SpawnError;

/// Hide a console that Windows created solely for this process.
///
/// A shared terminal is never hidden. Other platforms have no console window to manage and do nothing.
///
/// # Errors
///
/// [`SpawnError::ConsolePresentation`] when Windows cannot enumerate the processes attached to an existing
/// console. The caller keeps the console visible and reports the failure there.
pub fn hide_if_private() -> Result<(), SpawnError> {
    platform::hide_if_private()
}

#[cfg(any(windows, test))]
const fn is_private(attached_processes: u32) -> bool {
    attached_processes == 1
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};

    use super::{SpawnError, is_private};

    #[expect(
        unsafe_code,
        reason = "the console process list and window visibility are Windows process APIs with no safe wrapper. each call's process-owned arguments and lifetime are stated beside it"
    )]
    pub(super) fn hide_if_private() -> Result<(), SpawnError> {
        // SAFETY: `GetConsoleWindow` reads the calling process's console association and returns an OS-owned
        // handle. A null handle means no console exists, which is already the requested result.
        let window = unsafe { GetConsoleWindow() };
        if window.is_null() {
            return Ok(());
        }

        let mut process_ids = [0_u32; 2];
        // SAFETY: the pointer names the two-element writable array above for exactly its declared length. The
        // function writes process identifiers only and returns the full attached-process count.
        let attached = unsafe {
            GetConsoleProcessList(
                process_ids.as_mut_ptr(),
                u32::try_from(process_ids.len()).unwrap_or(2),
            )
        };
        if attached == 0 {
            return Err(SpawnError::ConsolePresentation {
                detail: std::io::Error::last_os_error().to_string(),
            });
        }
        if is_private(attached) {
            // SAFETY: `window` is the non-null handle returned for this process's current console above.
            // `ShowWindow` borrows it only for the call and `SW_HIDE` changes visibility without destroying it.
            unsafe { ShowWindow(window, SW_HIDE) };
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::SpawnError;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature on every platform. the Windows implementation can fail and this one cannot"
    )]
    pub(super) const fn hide_if_private() -> Result<(), SpawnError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_console_owned_by_this_process_is_private() {
        assert!(
            !is_private(0),
            "zero is an enumeration failure, not ownership"
        );
        assert!(is_private(1), "runtrol alone owns a launch-created console");
        assert!(
            !is_private(2),
            "a shell and runtrol share the operator's terminal"
        );
        assert!(!is_private(20), "every larger console is shared too");
    }
}
