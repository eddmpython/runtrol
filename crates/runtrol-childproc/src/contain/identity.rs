//! Kernel process identity used by durable Unix process-group recovery.

use std::path::{Path, PathBuf};

use crate::error::SpawnError;

/// Maximum executable path retained by one guard record.
pub(super) const MAX_EXECUTABLE_BYTES: usize = 32 * 1024;

/// An operating-system process identity that does not alias after PID reuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProcessIdentity {
    /// Process and process-group identifier. Every supervised root leads its own group.
    pub(super) pid: u32,
    /// Kernel-recorded process start value in the platform's native unit.
    pub(super) start: u64,
    /// Canonical executable of the stable keeper.
    pub(super) executable: PathBuf,
}

/// What the kernel says about the durable keeper generation now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveIdentity {
    /// PID, start value, and executable all still name the recorded keeper.
    Exact,
    /// No process currently has the recorded PID.
    Gone,
    /// The PID exists but its start value or executable belongs to another generation.
    Reused,
}

impl ProcessIdentity {
    /// Read the current process identity while the Unix bootstrap is still running.
    pub(super) fn current(expected: &Path) -> Result<Self, SpawnError> {
        let pid = std::process::id();
        let start =
            start_of(pid)?.ok_or_else(|| failure("reading the bootstrap start identity"))?;
        let executable = canonical_executable(expected)?;
        Ok(Self {
            pid,
            start,
            executable,
        })
    }

    /// Revalidate the stable keeper against every durable kernel identity field.
    pub(super) fn live_identity(&self) -> Result<LiveIdentity, SpawnError> {
        let Some(start) = start_of(self.pid)? else {
            return Ok(LiveIdentity::Gone);
        };
        if start != self.start {
            return Ok(LiveIdentity::Reused);
        }
        let Some(executable) = executable_of(self.pid)? else {
            return Ok(LiveIdentity::Gone);
        };
        if executable == self.executable {
            Ok(LiveIdentity::Exact)
        } else {
            Ok(LiveIdentity::Reused)
        }
    }
}

/// Canonicalize an executable before it becomes a durable comparison value.
pub(super) fn canonical_executable(path: &Path) -> Result<PathBuf, SpawnError> {
    let canonical = std::fs::canonicalize(path).map_err(|error| SpawnError::Containment {
        doing: "canonicalizing a supervised executable",
        detail: error.to_string(),
    })?;
    if canonical.as_os_str().as_encoded_bytes().len() > MAX_EXECUTABLE_BYTES {
        return Err(SpawnError::Containment {
            doing: "recording a supervised executable",
            detail: format!("the executable path exceeds {MAX_EXECUTABLE_BYTES} bytes"),
        });
    }
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn start_of(pid: u32) -> Result<Option<u64>, SpawnError> {
    let path = format!("/proc/{pid}/stat");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SpawnError::Containment {
                doing: "reading a process start identity",
                detail: error.to_string(),
            });
        }
    };
    let tail = text
        .rsplit_once(") ")
        .map(|(_, tail)| tail)
        .ok_or_else(|| failure("parsing a process start identity"))?;
    let start = tail
        .split_whitespace()
        // Field 22 overall. The tail begins at field 3, so start time is index 19.
        .nth(19)
        .ok_or_else(|| failure("parsing a process start identity"))?
        .parse::<u64>()
        .map_err(|error| SpawnError::Containment {
            doing: "parsing a process start identity",
            detail: error.to_string(),
        })?;
    Ok(Some(start))
}

#[cfg(target_os = "linux")]
fn executable_of(pid: u32) -> Result<Option<PathBuf>, SpawnError> {
    match std::fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SpawnError::Containment {
            doing: "reading a process executable identity",
            detail: error.to_string(),
        }),
    }
}

#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "macOS exposes process start and group identity only through proc_pidinfo"
)]
fn bsd_info(pid: u32) -> Result<Option<libc::proc_bsdinfo>, SpawnError> {
    let pid = i32::try_from(pid).map_err(|error| SpawnError::Containment {
        doing: "reading a process identity",
        detail: error.to_string(),
    })?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).unwrap_or(i32::MAX);
    // SAFETY: `info` points to writable storage of exactly the size passed to `proc_pidinfo`. The value is read only
    // when the kernel reports that it filled the complete structure.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
            Ok(None)
        } else {
            Err(SpawnError::Containment {
                doing: "reading a process identity",
                detail: error.to_string(),
            })
        };
    }
    if read != size {
        return Err(failure("reading a complete process identity"));
    }
    // SAFETY: the complete-size result above states that every byte of `info` was initialized by the kernel.
    Ok(Some(unsafe { info.assume_init() }))
}

#[cfg(target_os = "macos")]
fn start_of(pid: u32) -> Result<Option<u64>, SpawnError> {
    let Some(info) = bsd_info(pid)? else {
        return Ok(None);
    };
    let micros = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(info.pbi_start_tvusec))
        .ok_or_else(|| failure("combining a process start identity"))?;
    Ok(Some(micros))
}

#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "macOS exposes a live process executable path only through proc_pidpath"
)]
fn executable_of(pid: u32) -> Result<Option<PathBuf>, SpawnError> {
    use std::os::unix::ffi::OsStringExt as _;

    let pid = i32::try_from(pid).map_err(|error| SpawnError::Containment {
        doing: "reading a process executable identity",
        detail: error.to_string(),
    })?;
    let mut bytes = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let size = u32::try_from(bytes.len()).map_err(|error| SpawnError::Containment {
        doing: "reading a process executable identity",
        detail: error.to_string(),
    })?;
    // SAFETY: `bytes` is writable for exactly `size` bytes, and proc_pidpath borrows it only for this call.
    let written = unsafe { libc::proc_pidpath(pid, bytes.as_mut_ptr().cast(), size) };
    if written == 0 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
            Ok(None)
        } else {
            Err(SpawnError::Containment {
                doing: "reading a process executable identity",
                detail: error.to_string(),
            })
        };
    }
    let written = usize::try_from(written).map_err(|error| SpawnError::Containment {
        doing: "reading a process executable identity",
        detail: error.to_string(),
    })?;
    if written >= bytes.len() {
        return Err(failure("reading a complete process executable identity"));
    }
    bytes.truncate(written);
    Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("durable Unix containment currently supports Linux and macOS only");

fn failure(doing: &'static str) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: "the operating system returned an incomplete process identity".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_keeper_identity_revalidates_exactly() -> Result<(), SpawnError> {
        let executable = std::env::current_exe().map_err(|error| SpawnError::Containment {
            doing: "finding the identity test executable",
            detail: error.to_string(),
        })?;
        let identity = ProcessIdentity::current(&executable)?;

        assert_eq!(identity.live_identity()?, LiveIdentity::Exact);
        Ok(())
    }

    #[test]
    fn executable_mismatch_is_not_an_exact_keeper() -> Result<(), SpawnError> {
        let executable = std::env::current_exe().map_err(|error| SpawnError::Containment {
            doing: "finding the identity test executable",
            detail: error.to_string(),
        })?;
        let mut identity = ProcessIdentity::current(&executable)?;
        identity.executable.push("not-the-live-keeper");

        assert_eq!(identity.live_identity()?, LiveIdentity::Reused);
        Ok(())
    }
}
