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
    /// Canonical executable expected after the bootstrap replaces itself.
    pub(super) executable: PathBuf,
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

    /// Whether the PID still names this exact process and still leads its recorded group.
    pub(super) fn matches_live_root(&self) -> Result<bool, SpawnError> {
        let Some(start) = start_of(self.pid)? else {
            return Ok(false);
        };
        if start != self.start || process_group_of(self.pid)? != Some(self.pid) {
            return Ok(false);
        }
        let Some(executable) = executable_of(self.pid)? else {
            return Ok(false);
        };
        if executable != self.executable {
            return Ok(false);
        }
        Ok(start_of(self.pid)? == Some(self.start))
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
fn process_group_of(pid: u32) -> Result<Option<u32>, SpawnError> {
    let path = format!("/proc/{pid}/stat");
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SpawnError::Containment {
                doing: "reading a process group",
                detail: error.to_string(),
            });
        }
    };
    let tail = text
        .rsplit_once(") ")
        .map(|(_, tail)| tail)
        .ok_or_else(|| failure("parsing a process group"))?;
    let group = tail
        .split_whitespace()
        // The tail begins at state, followed by ppid and process group.
        .nth(2)
        .ok_or_else(|| failure("parsing a process group"))?
        .parse::<u32>()
        .map_err(|error| SpawnError::Containment {
            doing: "parsing a process group",
            detail: error.to_string(),
        })?;
    Ok(Some(group))
}

#[cfg(target_os = "linux")]
fn executable_of(pid: u32) -> Result<Option<PathBuf>, SpawnError> {
    match std::fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SpawnError::Containment {
            doing: "reading a process executable",
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
fn process_group_of(pid: u32) -> Result<Option<u32>, SpawnError> {
    Ok(bsd_info(pid)?.map(|info| info.pbi_pgid))
}

#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "macOS exposes the executable for an arbitrary PID only through proc_pidpath"
)]
fn executable_of(pid: u32) -> Result<Option<PathBuf>, SpawnError> {
    use std::os::unix::ffi::OsStringExt as _;

    let pid = i32::try_from(pid).map_err(|error| SpawnError::Containment {
        doing: "reading a process executable",
        detail: error.to_string(),
    })?;
    let mut bytes = vec![0_u8; usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).unwrap_or(4096)];
    // SAFETY: `bytes` is writable for its declared length and that same length is passed to the kernel.
    let read = unsafe {
        libc::proc_pidpath(
            pid,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
            Ok(None)
        } else {
            Err(SpawnError::Containment {
                doing: "reading a process executable",
                detail: error.to_string(),
            })
        };
    }
    let length = usize::try_from(read).map_err(|error| SpawnError::Containment {
        doing: "reading a process executable",
        detail: error.to_string(),
    })?;
    bytes.truncate(length);
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
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
