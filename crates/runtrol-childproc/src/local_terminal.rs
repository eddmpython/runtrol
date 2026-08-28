//! Borrowing the invoking terminal as a byte-transparent viewer of a daemon-owned PTY.
//!
//! A transparent provider bridge must stop the local console from interpreting line editing, echo, or interrupt
//! characters. The provider process does not own this terminal, so those exact bytes travel to the one PTY owned by
//! the daemon. The guard restores every changed mode on drop, including early transport failure.

use std::io;

/// The visible cell geometry of the invoking terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTerminalSize {
    /// Columns visible to the operator.
    pub cols: u16,
    /// Rows visible to the operator.
    pub rows: u16,
}

/// A raw-mode lease on the invoking terminal.
pub struct LocalTerminal {
    platform: platform::LocalTerminal,
}

impl LocalTerminal {
    /// Put the invoking terminal in raw byte mode and remember enough state to restore it.
    ///
    /// # Errors
    ///
    /// Returns the platform error when standard input or output is not an attached terminal, or its mode cannot be
    /// read or changed.
    pub fn acquire() -> io::Result<Self> {
        platform::LocalTerminal::acquire().map(|platform| Self { platform })
    }

    /// Read the current visible geometry.
    ///
    /// # Errors
    ///
    /// Returns the platform error when the terminal no longer exposes its screen dimensions.
    pub fn size(&self) -> io::Result<LocalTerminalSize> {
        self.platform.size()
    }
}

#[cfg(windows)]
mod platform {
    use std::io;

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
        GetConsoleScreenBufferInfo, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };

    use super::LocalTerminalSize;

    pub(super) struct LocalTerminal {
        input: HANDLE,
        output: HANDLE,
        input_mode: u32,
        output_mode: u32,
    }

    impl LocalTerminal {
        #[expect(
            unsafe_code,
            reason = "console handles and mode changes have no safe standard-library interface"
        )]
        pub(super) fn acquire() -> io::Result<Self> {
            // SAFETY: the calls return process-owned standard handles and write only to initialized mode values.
            unsafe {
                let input = GetStdHandle(STD_INPUT_HANDLE);
                let output = GetStdHandle(STD_OUTPUT_HANDLE);
                valid_handle(input)?;
                valid_handle(output)?;
                let mut input_mode = 0_u32;
                let mut output_mode = 0_u32;
                if GetConsoleMode(input, &raw mut input_mode) == 0
                    || GetConsoleMode(output, &raw mut output_mode) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let raw_input = (input_mode
                    & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT;
                let raw_output = output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                if SetConsoleMode(input, raw_input) == 0 {
                    return Err(io::Error::last_os_error());
                }
                if SetConsoleMode(output, raw_output) == 0 {
                    let _restored = SetConsoleMode(input, input_mode);
                    return Err(io::Error::last_os_error());
                }
                Ok(Self {
                    input,
                    output,
                    input_mode,
                    output_mode,
                })
            }
        }

        #[expect(
            unsafe_code,
            reason = "reading the console screen buffer geometry has no safe standard-library interface"
        )]
        pub(super) fn size(&self) -> io::Result<LocalTerminalSize> {
            // SAFETY: `self.output` remains process-owned for the guard lifetime and the call initializes `info`.
            unsafe {
                let mut info = std::mem::zeroed();
                if GetConsoleScreenBufferInfo(self.output, &raw mut info) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let cols = info.srWindow.Right - info.srWindow.Left + 1;
                let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
                let cols = u16::try_from(cols).map_err(|_| invalid_geometry())?;
                let rows = u16::try_from(rows).map_err(|_| invalid_geometry())?;
                if cols == 0 || rows == 0 {
                    return Err(invalid_geometry());
                }
                Ok(LocalTerminalSize { cols, rows })
            }
        }
    }

    impl Drop for LocalTerminal {
        #[expect(
            unsafe_code,
            reason = "restoring saved console modes has no safe standard-library interface"
        )]
        fn drop(&mut self) {
            // SAFETY: both handles and both saved modes came from successful console calls in `acquire`.
            unsafe {
                let _input_restored = SetConsoleMode(self.input, self.input_mode);
                let _output_restored = SetConsoleMode(self.output, self.output_mode);
            }
        }
    }

    fn valid_handle(handle: HANDLE) -> io::Result<()> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn invalid_geometry() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the terminal geometry is invalid",
        )
    }
}

#[cfg(unix)]
mod platform {
    use std::io;

    use super::LocalTerminalSize;

    pub(super) struct LocalTerminal {
        input_mode: libc::termios,
    }

    impl LocalTerminal {
        #[expect(
            unsafe_code,
            reason = "termios raw mode has no safe standard-library interface"
        )]
        pub(super) fn acquire() -> io::Result<Self> {
            // SAFETY: both calls address the current process's standard input and initialized termios values.
            unsafe {
                let mut input_mode = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &raw mut input_mode) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut raw = input_mode;
                libc::cfmakeraw(&raw mut raw);
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw const raw) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self { input_mode })
            }
        }

        #[expect(
            unsafe_code,
            reason = "the terminal window-size ioctl has no safe standard-library interface"
        )]
        pub(super) fn size(&self) -> io::Result<LocalTerminalSize> {
            // SAFETY: the ioctl writes one winsize value for the current process's standard output terminal.
            unsafe {
                let mut size: libc::winsize = std::mem::zeroed();
                if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if size.ws_col == 0 || size.ws_row == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the terminal geometry is invalid",
                    ));
                }
                Ok(LocalTerminalSize {
                    cols: size.ws_col,
                    rows: size.ws_row,
                })
            }
        }
    }

    impl Drop for LocalTerminal {
        #[expect(
            unsafe_code,
            reason = "restoring the saved termios state has no safe standard-library interface"
        )]
        fn drop(&mut self) {
            // SAFETY: the saved mode came from this standard input in `acquire` and remains initialized.
            unsafe {
                let _restored = libc::tcsetattr(
                    libc::STDIN_FILENO,
                    libc::TCSANOW,
                    &raw const self.input_mode,
                );
            }
        }
    }
}
