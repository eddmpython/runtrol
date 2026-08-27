//! Kernel process identity used by durable Unix process-group recovery.
//!
//! The executable is deliberately not part of this identity. An update replaces the file behind a
//! live keeper without touching the process, so an executable lookup names a deleted or different
//! path while the recorded keeper is still ours (measured: rename-over marks every live lookup
//! `(deleted)` on Linux). PID plus kernel start value identifies a process within one boot, and the
//! boot identifier closes the reboot boundary that the executable field covered by accident.

use crate::error::SpawnError;

/// Fixed width of one recorded boot identifier.
pub(super) const BOOT_ID_BYTES: usize = 16;

/// Maximum executable-path bytes accepted while decoding a previous-format guard record.
pub(super) const MAX_EXECUTABLE_BYTES: usize = 32 * 1024;

/// An operating-system process identity that does not alias after PID reuse or an executable update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProcessIdentity {
    /// Process and process-group identifier. Every supervised root leads its own group.
    pub(super) pid: u32,
    /// Kernel-recorded process start value in the platform's native unit.
    pub(super) start: u64,
    /// The boot this identity was recorded in. `None` only for records written by the previous
    /// format, which cannot separate a reboot; those resolve through the group check instead.
    pub(super) boot: Option<[u8; BOOT_ID_BYTES]>,
}

/// What the kernel says about the durable keeper generation now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveIdentity {
    /// Boot, PID, and start value still name the recorded keeper, even if its executable was
    /// replaced on disk since.
    Ours,
    /// No process currently has the recorded PID.
    Gone,
    /// The record was written in a different boot, so nothing recorded then can still execute.
    /// This is the only state whose record may be removed without a group check.
    DifferentBoot,
    /// The PID exists but its start value belongs to another generation. The kernel reallocates a
    /// number only once nothing held it, but that reasoning was measured on one emulated surface,
    /// so recovery treats the record as ambiguous instead of trusting the reallocation argument.
    Ambiguous,
}

impl ProcessIdentity {
    /// Read the current process identity while the Unix bootstrap is still running.
    pub(super) fn current() -> Result<Self, SpawnError> {
        let pid = std::process::id();
        let start =
            start_of(pid)?.ok_or_else(|| failure("reading the bootstrap start identity"))?;
        Ok(Self {
            pid,
            start,
            boot: Some(current_boot()?),
        })
    }

    /// Revalidate the stable keeper against every durable kernel identity field.
    pub(super) fn live_identity(&self) -> Result<LiveIdentity, SpawnError> {
        if let Some(boot) = self.boot
            && boot != current_boot()?
        {
            return Ok(LiveIdentity::DifferentBoot);
        }
        let Some(start) = start_of(self.pid)? else {
            return Ok(LiveIdentity::Gone);
        };
        if start == self.start {
            Ok(LiveIdentity::Ours)
        } else {
            Ok(LiveIdentity::Ambiguous)
        }
    }
}

fn current_boot() -> Result<[u8; BOOT_ID_BYTES], SpawnError> {
    static BOOT: std::sync::OnceLock<[u8; BOOT_ID_BYTES]> = std::sync::OnceLock::new();
    if let Some(boot) = BOOT.get() {
        return Ok(*boot);
    }
    let boot = read_boot_id()?;
    Ok(*BOOT.get_or_init(|| boot))
}

#[cfg(target_os = "linux")]
fn read_boot_id() -> Result<[u8; BOOT_ID_BYTES], SpawnError> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|error| {
        SpawnError::Containment {
            doing: "reading the boot identity",
            detail: error.to_string(),
        }
    })?;
    let mut bytes = [0_u8; BOOT_ID_BYTES];
    let mut digits = text.trim().bytes().filter(u8::is_ascii_hexdigit);
    for slot in &mut bytes {
        let high = digits
            .next()
            .ok_or_else(|| failure("parsing the boot identity"))?;
        let low = digits
            .next()
            .ok_or_else(|| failure("parsing the boot identity"))?;
        *slot = (hex_value(high) << 4) | hex_value(low);
    }
    if digits.next().is_some() {
        return Err(failure("parsing the boot identity"));
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
const fn hex_value(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        _ => digit - b'A' + 10,
    }
}

#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "macOS exposes the boot moment only through the kern.boottime sysctl"
)]
fn read_boot_id() -> Result<[u8; BOOT_ID_BYTES], SpawnError> {
    let mut boottime = std::mem::MaybeUninit::<libc::timeval>::uninit();
    let mut size = size_of::<libc::timeval>();
    let mut name = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    // SAFETY: `boottime` is writable for exactly `size` bytes and the kernel borrows the name array
    // only for this call. The value is read only after a complete-size success.
    let result = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            2,
            boottime.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || size != size_of::<libc::timeval>() {
        return Err(failure("reading the boot identity"));
    }
    // SAFETY: the complete-size result above states the kernel initialized every byte.
    let boottime = unsafe { boottime.assume_init() };
    let mut bytes = [0_u8; BOOT_ID_BYTES];
    // `timeval` field widths differ per platform (Apple: i64 seconds, i32 microseconds; Linux: i64 both),
    // so one of these widenings is the identity on some target and a real widening on another.
    #[allow(
        clippy::useless_conversion,
        reason = "identity on the platforms whose fields are already i64"
    )]
    let seconds = i64::from(boottime.tv_sec);
    #[allow(
        clippy::useless_conversion,
        reason = "identity on the platforms whose fields are already i64"
    )]
    let microseconds = i64::from(boottime.tv_usec);
    bytes[..8].copy_from_slice(&seconds.to_le_bytes());
    bytes[8..].copy_from_slice(&microseconds.to_le_bytes());
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn start_of(pid: u32) -> Result<Option<u64>, SpawnError> {
    let path = format!("/proc/{pid}/stat");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if process_lookup_is_gone(&error) => return Ok(None),
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
fn process_lookup_is_gone(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
        || matches!(error.raw_os_error(), Some(libc::ESRCH))
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
    fn current_keeper_identity_revalidates_as_ours() -> Result<(), SpawnError> {
        let identity = ProcessIdentity::current()?;

        assert_eq!(identity.live_identity()?, LiveIdentity::Ours);
        Ok(())
    }

    #[test]
    fn a_replaced_executable_cannot_change_the_verdict() -> Result<(), SpawnError> {
        // The executable is not consulted at all, which is the point: an update that renames the
        // file behind this live process must leave the keeper recognized as ours.
        let identity = ProcessIdentity::current()?;

        assert_eq!(identity.live_identity()?, LiveIdentity::Ours);
        Ok(())
    }

    #[test]
    fn a_start_value_from_another_generation_is_ambiguous() -> Result<(), SpawnError> {
        let mut identity = ProcessIdentity::current()?;
        identity.start = identity.start.wrapping_add(1);

        assert_eq!(identity.live_identity()?, LiveIdentity::Ambiguous);
        Ok(())
    }

    #[test]
    fn a_record_from_another_boot_is_closed_without_a_group_check() -> Result<(), SpawnError> {
        let mut identity = ProcessIdentity::current()?;
        let mut boot = identity.boot.expect("current identity carries a boot");
        boot[0] = boot[0].wrapping_add(1);
        identity.boot = Some(boot);

        assert_eq!(identity.live_identity()?, LiveIdentity::DifferentBoot);
        Ok(())
    }

    #[test]
    fn a_previous_format_record_without_a_boot_still_resolves() -> Result<(), SpawnError> {
        let mut identity = ProcessIdentity::current()?;
        identity.boot = None;

        assert_eq!(identity.live_identity()?, LiveIdentity::Ours);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_esrch_process_lookup_is_a_gone_process() {
        let error = std::io::Error::from_raw_os_error(libc::ESRCH);

        assert!(process_lookup_is_gone(&error));
    }
}
