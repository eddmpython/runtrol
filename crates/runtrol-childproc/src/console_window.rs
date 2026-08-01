//! Keeping background children from opening a Windows console window.
//!
//! The desktop process can hide the console it owns, but that does not govern a console program started later.
//! Every provider process and short discovery probe passes through this policy before spawn. Terminal launches
//! remain unchanged because this flag applies to children supervised in the background, not to runtrol itself.

use std::process::Command;

/// Prevent `command` from creating a visible console window on Windows.
///
/// This changes only window creation. Standard input, output, error, process grouping, and containment remain
/// the caller's decisions. It is a no-op on platforms where a Windows console cannot be created.
#[cfg(windows)]
pub fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    /// Run a console application without creating or inheriting a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    command.creation_flags(CREATE_NO_WINDOW);
}

/// Leave process creation unchanged where Windows console windows do not exist.
#[cfg(not(windows))]
pub fn hide_console_window(_command: &mut Command) {}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;
    use std::process::Stdio;

    use windows_sys::Win32::System::Console::GetConsoleWindow;

    const PROBE_ENV: &str = "RUNTROL_CONSOLE_WINDOW_PROBE";
    const TEST_NAME: &str = "console_window::tests::hidden_child_has_no_console_window";

    #[test]
    #[expect(
        unsafe_code,
        reason = "the Windows console association has no safe query API; the call takes no arguments and the safety argument is beside it"
    )]
    fn hidden_child_has_no_console_window() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var_os(PROBE_ENV).is_some() {
            // SAFETY: `GetConsoleWindow` takes no pointers or handles and only reports the calling process's
            // console association.
            let window = unsafe { GetConsoleWindow() };
            assert!(
                window.is_null(),
                "a child created with the background policy still owns a console window"
            );
            return Ok(());
        }

        let own_path = std::env::current_exe()?;
        let mut child = Command::new(own_path);
        child
            .args(["--exact", TEST_NAME])
            .env(PROBE_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_console_window(&mut child);

        let status = child.status()?;
        assert!(
            status.success(),
            "the hidden console probe failed: {status}"
        );
        Ok(())
    }
}
