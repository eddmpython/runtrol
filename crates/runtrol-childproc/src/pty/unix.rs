//! `openpty` plus the standard library's own process spawn: the slave becomes the child's standard streams
//! and its controlling terminal, the master stays here as one file.
//!
//! The child is started through `std::process::Command`, so [`crate::contain`] applies to it the same way it
//! applies to every other child, and the only work done between fork and exec is the two async-signal-safe
//! calls that make the slave the controlling terminal.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{PtySize, PtySpawn, child_environment, full_arguments};
use crate::error::SpawnError;

/// The Unix half of [`super::PtyChild`].
#[derive(Debug)]
pub(super) struct Child {
    master: File,
    /// The child's pid. The `std::process::Child` is dropped at spawn: it would only be a second owner of the
    /// same pid, and the platform calls below need nothing from it.
    pid: libc::pid_t,
    /// Whether the exit status has been collected, so a reaped pid is never waited on again.
    reaped: AtomicBool,
}

fn errno(doing: &'static str) -> SpawnError {
    SpawnError::Pty {
        doing,
        detail: std::io::Error::last_os_error().to_string(),
    }
}

fn winsize(size: PtySize) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

impl Child {
    #[expect(
        unsafe_code,
        reason = "openpty, the controlling-terminal ioctl and taking ownership of the descriptors have no safe wrapper"
    )]
    pub(super) fn spawn(spawn: PtySpawn<'_>) -> Result<Self, SpawnError> {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = winsize(spawn.size);
        // SAFETY: both out-pointers are live locals; the name pointer may be null; the termios pointer may be
        // null; the winsize pointer is a live local.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                core::ptr::null_mut(),
                core::ptr::null(),
                &raw mut size,
            )
        };
        if opened != 0 {
            return Err(errno("opening the pseudo terminal"));
        }
        let (master, slave) =
            // SAFETY: both descriptors were just returned by `openpty` and are owned by nothing else.
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

        let mut command = Command::new(spawn.program.path().as_str());
        command
            .args(full_arguments(&spawn))
            .current_dir(spawn.cwd.as_str())
            .env_clear()
            .envs(child_environment(&spawn))
            .stdin(Stdio::from(slave.try_clone().map_err(|error| {
                SpawnError::Pty {
                    doing: "duplicating the terminal for the child",
                    detail: error.to_string(),
                }
            })?))
            .stdout(Stdio::from(slave.try_clone().map_err(|error| {
                SpawnError::Pty {
                    doing: "duplicating the terminal for the child",
                    detail: error.to_string(),
                }
            })?))
            .stderr(Stdio::from(slave));
        // SAFETY: the closure runs between fork and exec and makes only async-signal-safe calls: `setsid`
        // and one `ioctl` on descriptor 0, which is the slave the standard library just installed.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|error| SpawnError::Pty {
            doing: "starting the program on the pseudo terminal",
            detail: error.to_string(),
        })?;
        let pid = libc::pid_t::try_from(child.id()).map_err(|error| SpawnError::Pty {
            doing: "reading the child's pid",
            detail: error.to_string(),
        })?;
        drop(child);
        Ok(Self {
            master: File::from(master),
            pid,
            reaped: AtomicBool::new(false),
        })
    }

    pub(super) fn pid(&self) -> u32 {
        self.pid.cast_unsigned()
    }

    /// Nothing to close beyond the child: the master reports end of stream once the slave's last holder
    /// exits. Present so both platforms have the same shape.
    pub(super) fn finish(&self) {}

    pub(super) fn reader(&self) -> Result<Box<dyn Read + Send>, SpawnError> {
        let file = self.master.try_clone().map_err(|error| SpawnError::Pty {
            doing: "duplicating the terminal output",
            detail: error.to_string(),
        })?;
        Ok(Box::new(file))
    }

    pub(super) fn writer(&self) -> Result<Box<dyn Write + Send>, SpawnError> {
        let file = self.master.try_clone().map_err(|error| SpawnError::Pty {
            doing: "duplicating the terminal input",
            detail: error.to_string(),
        })?;
        Ok(Box::new(file))
    }

    #[expect(unsafe_code, reason = "the window-size ioctl has no safe wrapper")]
    pub(super) fn resize(&self, size: PtySize) -> Result<(), SpawnError> {
        let size = winsize(size);
        let result =
            // SAFETY: the descriptor is the open master; the request takes a pointer to a live `winsize`.
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
        if result < 0 {
            return Err(errno("resizing the pseudo terminal"));
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "waitpid has no safe wrapper for a pid this type owns"
    )]
    pub(super) fn try_wait(&self) -> Result<Option<i32>, SpawnError> {
        if self.reaped.load(Ordering::SeqCst) {
            return Ok(Some(0));
        }
        let mut status: libc::c_int = 0;
        // SAFETY: the pid is a child of this process that has not been reaped (the flag above); the status
        // pointer is a live local; WNOHANG makes this a poll.
        let waited = unsafe { libc::waitpid(self.pid, &raw mut status, libc::WNOHANG) };
        if waited == 0 {
            return Ok(None);
        }
        if waited < 0 {
            return Err(errno("reading the exit status"));
        }
        self.reaped.store(true, Ordering::SeqCst);
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            -libc::WTERMSIG(status)
        };
        Ok(Some(code))
    }

    #[expect(
        unsafe_code,
        reason = "kill has no safe wrapper for a pid this type owns"
    )]
    pub(super) fn kill(&self) -> Result<(), SpawnError> {
        if self.reaped.load(Ordering::SeqCst) {
            return Ok(());
        }
        // SAFETY: the pid is a live, unreaped child of this process, so it cannot have been reused.
        let sent = unsafe { libc::kill(self.pid, libc::SIGKILL) };
        if sent < 0 {
            return Err(errno("terminating the program"));
        }
        Ok(())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // Reported nowhere on purpose: `Drop` has no error channel, and a process that already ended makes
        // this call fail by design.
        drop(self.kill());
    }
}
