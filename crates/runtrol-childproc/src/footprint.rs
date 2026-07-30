//! How much memory a process is actually holding.
//!
//! # Why this exists at all
//!
//! Because "memory efficient" is not an adjective here, it is a number with a gate behind it. A budget nobody can
//! measure is a budget nobody is held to, and the first release that quietly doubles is the one where somebody
//! discovers the claim was decoration.
//!
//! It is also something an operator asks. runtrol sits resident on their machine all day; what it costs them to
//! leave running is a fair question, and a product that cannot answer it is asking for trust it has not earned.
//!
//! # What is measured
//!
//! The resident set: the part of a process that is really in memory now. Not the address space it reserved, which
//! on a 64-bit machine says almost nothing, and not what an allocator thinks it handed out, which is a number
//! from inside the process about the process and can be wrong in the direction that flatters it.
//!
//! Measured from outside, by asking the operating system about a process by identifier. That is what makes it
//! usable on the daemon from a test that is not the daemon.
//!
//! # Why each platform is asked differently
//!
//! Because each one answers differently, and the alternative is one wrong abstraction. Windows has a documented
//! call and no file. Linux has a file and no need for a call. macOS has neither in a form this crate can reach
//! without more `unsafe` than the number is worth, so it asks the tool that ships with the system. Each is the
//! cheapest correct answer on its own platform, and none of them pretends to be the others.

use crate::error::SpawnError;

/// How much memory a process is holding, in bytes.
///
/// `pid` is the process to ask about. Asking about this process is allowed and is what a daemon reporting its own
/// cost would do.
///
/// # Errors
///
/// [`SpawnError::Footprint`] when the platform will not answer: the process ended between being named and being
/// asked about, or this platform has no way to ask that this crate can reach.
pub fn resident_bytes(pid: u32) -> Result<u64, SpawnError> {
    platform::resident_bytes(pid)
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    use crate::error::SpawnError;

    #[expect(
        unsafe_code,
        reason = "opening a process and asking the kernel its working set are calls with no safe wrapper. the safety argument is at each block"
    )]
    pub(super) fn resident_bytes(pid: u32) -> Result<u64, SpawnError> {
        // The least access that answers this question, so a process this program may not fully open is still
        // measurable where the platform allows the narrower right. Bound here rather than written into the call
        // so that the call fits one line, which is what keeps the argument below directly above its `unsafe`.
        let rights = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;

        // SAFETY: `OpenProcess` takes an access mask, an inheritance flag and an identifier, and returns a handle
        // or null. It reads nothing of ours, and the null case is checked immediately below.
        let process = unsafe { OpenProcess(rights, 0, pid) };
        if process.is_null() {
            return Err(SpawnError::Footprint {
                pid,
                detail: std::io::Error::last_os_error().to_string(),
            });
        }

        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: 0,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).unwrap_or(0);
        counters.cb = size;

        // SAFETY: `process` came from the checked `OpenProcess` above. The structure is fully initialised, lives
        // for the whole call, and its `cb` field says how large it is, which is how the call knows what it may
        // write. The pointer is to that local and to nothing shared.
        let asked = unsafe { GetProcessMemoryInfo(process, &raw mut counters, size) };

        // SAFETY: `process` is the handle from above and nothing else holds it. Closed before the result is
        // read, so a failure to answer does not leak a handle.
        //
        // Its own result is not checked, and that is safe rather than swallowed: closing a handle this function
        // opened moments ago fails only if it was already invalid, which cannot be true here, and there is
        // nothing a caller asking about memory could do about it either way. The measurement below is what this
        // function is for, and it is checked.
        let _closed = unsafe { CloseHandle(process) };

        if asked == 0 {
            return Err(SpawnError::Footprint {
                pid,
                detail: std::io::Error::last_os_error().to_string(),
            });
        }
        Ok(counters.WorkingSetSize as u64)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use crate::error::SpawnError;

    /// What the kernel calls a page, in bytes.
    ///
    /// `statm` counts pages, so this is what turns its answer into bytes. Asked rather than assumed: the common
    /// answer is 4 KiB and it is not the only one, and a machine with larger pages would otherwise be reported at
    /// a fraction of what it is holding, which is the direction a budget gate must never be wrong in.
    ///
    /// Read from a file, so no `unsafe` is involved. The kernel reports it on every mapping in `smaps`, and the
    /// first one is enough because it is a property of the kernel rather than of the mapping.
    fn page_bytes() -> u64 {
        /// What to fall back to, and the only case it is used in.
        ///
        /// A kernel too old to report the size in `smaps` is one where 4 KiB is what it was. Stated here rather
        /// than buried, because a fallback that goes unmentioned is a number nobody can check.
        const WHEN_UNREPORTED: u64 = 4096;

        // Each step falls back rather than failing, and every one of them is the same case: this kernel does not
        // report the size, so it is the one where 4 KiB was the answer. Written as explicit branches so that
        // nothing here is an error being dropped on the way past.
        let Ok(text) = std::fs::read_to_string("/proc/self/smaps") else {
            return WHEN_UNREPORTED;
        };
        let Some(value) = text
            .lines()
            .find_map(|line| line.strip_prefix("KernelPageSize:"))
        else {
            return WHEN_UNREPORTED;
        };
        let Some(number) = value.split_whitespace().next() else {
            return WHEN_UNREPORTED;
        };
        match number.parse::<u64>() {
            Ok(kibibytes) => kibibytes.saturating_mul(1024).max(WHEN_UNREPORTED),
            Err(_) => WHEN_UNREPORTED,
        }
    }

    pub(super) fn resident_bytes(pid: u32) -> Result<u64, SpawnError> {
        let path = format!("/proc/{pid}/statm");
        let text = std::fs::read_to_string(&path).map_err(|error| SpawnError::Footprint {
            pid,
            detail: format!("{path}: {error}"),
        })?;

        // The second field is the resident set, in pages. The first is the address space, which says almost
        // nothing on a 64-bit machine and is the number people reach for by mistake.
        let unreadable = || SpawnError::Footprint {
            pid,
            detail: format!("{path} did not have a resident field to read"),
        };
        let field = text.split_whitespace().nth(1).ok_or_else(unreadable)?;
        let resident = field.parse::<u64>().map_err(|_| unreadable())?;

        Ok(resident.saturating_mul(page_bytes()))
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use crate::error::SpawnError;

    pub(super) fn resident_bytes(pid: u32) -> Result<u64, SpawnError> {
        // The tool that ships with the system, which answers in kibibytes. Reaching for the platform's own call
        // would need more `unsafe` than one number is worth, and a wrong answer here is worse than no answer:
        // a budget gate that measures the wrong thing is a budget gate that passes while the product grows.
        let asked = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .map_err(|error| SpawnError::Footprint {
                pid,
                detail: format!("ps: {error}"),
            })?;

        let text = String::from_utf8_lossy(&asked.stdout);
        let kibibytes = text
            .trim()
            .parse::<u64>()
            .map_err(|_| SpawnError::Footprint {
                pid,
                detail: format!(
                    "ps answered {:?}, which is not a number of kibibytes",
                    text.trim()
                ),
            })?;

        Ok(kibibytes.saturating_mul(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_holding_something() {
        // A process that is running holds memory. Zero would mean the platform answered and the answer is not
        // what this thinks it is, which is the failure a budget gate cannot survive: it would pass forever.
        let held =
            resident_bytes(std::process::id()).expect("this platform can be asked about a process");
        assert!(
            held > 1024 * 1024,
            "a running test process holding {held} bytes is not a measurement"
        );
        assert!(
            held < 64 * 1024 * 1024 * 1024,
            "a running test process holding {held} bytes is not a measurement either"
        );
    }

    #[test]
    fn a_process_that_is_not_there_is_a_failure_and_not_a_zero() {
        // Reporting nothing for a process that ended would let a budget gate pass by measuring a corpse. The
        // identifier below is beyond what any platform here assigns.
        match resident_bytes(u32::MAX - 1) {
            Err(SpawnError::Footprint { pid, .. }) => assert_eq!(pid, u32::MAX - 1),
            Ok(held) => panic!("a process that is not there reported {held} bytes"),
            Err(other) => panic!("expected a footprint failure naming the process, got {other}"),
        }
    }
}
