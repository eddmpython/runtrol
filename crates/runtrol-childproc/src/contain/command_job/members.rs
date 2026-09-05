//! Seal command membership before retaining the kernel objects that cleanup must wait for.

use std::os::windows::io::{AsRawHandle as _, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::JobObjects::{
    IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
};

use super::{failure, last_error, owned};
use crate::SpawnError;

// An excessive tree is terminated but cannot release a caller lease without complete ownership proof.
const MAX_RETAINED_PROCESSES: usize = 4096;
const SYNCHRONIZE: u32 = 0x0010_0000;

#[repr(C)]
struct ProcessList {
    assigned: u32,
    count: u32,
    ids: [usize; MAX_RETAINED_PROCESSES],
}

impl ProcessList {
    #[expect(
        unsafe_code,
        reason = "initialize the integer-only API buffer directly on the heap"
    )]
    fn empty() -> Box<Self> {
        let mut list = Box::<Self>::new_uninit();
        // SAFETY: this aligned allocation holds one ProcessList. Every field is an integer, so zero
        // initializes the complete buffer without creating a large stack temporary.
        unsafe { list.as_mut_ptr().write_bytes(0, 1) };
        // SAFETY: every byte in the complete ProcessList was initialized above.
        unsafe { list.assume_init() }
    }

    fn listed_ids(&self) -> Result<&[usize], SpawnError> {
        // A signaled process can leave the PID list before the assigned count catches up. The API's
        // successful query fills the available list; unequal counts alone do not mean truncation.
        // Preserve the capacity check even on success, and never index beyond the returned count.
        if usize::try_from(self.assigned).map_or(true, |assigned| assigned > self.ids.len())
            || self.count > self.assigned
        {
            return Err(failure(
                "retaining command Job members",
                format!(
                    "invalid bounded list: assigned {}, returned {}, capacity {MAX_RETAINED_PROCESSES}",
                    self.assigned, self.count
                ),
            ));
        }
        let count = usize::try_from(self.count)
            .map_err(|_| failure("retaining command Job members", "invalid member count"))?;
        self.ids.get(..count).ok_or_else(|| {
            failure(
                "retaining command Job members",
                "the member count exceeds its buffer",
            )
        })
    }
}

#[expect(
    unsafe_code,
    reason = "membership sealing and retained process handles are Windows kernel operations"
)]
pub(super) fn seal(job: HANDLE) -> Result<Vec<OwnedHandle>, SpawnError> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // With the flag present, zero rejects every subsequent assignment, including inherited children.
    // The real Windows populated-Job test also proves existing members stay available for exact waits.
    limits.BasicLimitInformation.ActiveProcessLimit = 0;
    // SAFETY: job is borrowed from the owning CommandJob; the class and initialized buffer agree.
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            core::ptr::from_ref(&limits).cast(),
            u32::try_from(size_of_val(&limits)).unwrap_or(0),
        )
    } == 0
    {
        return Err(last_error("sealing command Job membership"));
    }
    let mut list = ProcessList::empty();
    // SAFETY: this repr(C) buffer follows the documented variable-length process-list layout.
    if unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            core::ptr::from_mut(list.as_mut()).cast(),
            u32::try_from(size_of::<ProcessList>()).unwrap_or(0),
            core::ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error("retaining command Job members"));
    }
    let ids = list.listed_ids()?;
    let mut handles = Vec::with_capacity(ids.len());
    for &id in ids {
        let pid = u32::try_from(id).map_err(|_| {
            failure(
                "retaining command Job members",
                "invalid process identifier",
            )
        })?;
        // SAFETY: query and wait rights only; no process addressed here is mutated by identifier.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            if error
                .raw_os_error()
                .is_some_and(|code| u32::try_from(code) == Ok(ERROR_INVALID_PARAMETER))
            {
                // The process object no longer exists. Access denial is never interpreted as exit.
                continue;
            }
            return Err(failure(
                "opening a sealed command member",
                error.to_string(),
            ));
        }
        let handle = owned(raw, "retaining a sealed command member")?;
        let mut member = 0;
        // SAFETY: both handles are live. Membership revalidation excludes a PID reused after snapshot.
        if unsafe { IsProcessInJob(handle.as_raw_handle(), job, &raw mut member) } == 0 {
            return Err(last_error("validating a sealed command member"));
        }
        if member != 0 {
            handles.push(handle);
        }
    }
    Ok(handles)
}

#[expect(
    unsafe_code,
    reason = "zero-time waits inspect the exact retained kernel process objects"
)]
pub(super) fn stopped(handles: &[OwnedHandle]) -> Result<bool, SpawnError> {
    for handle in handles {
        // SAFETY: every handle is owned and opened for synchronization; a zero wait cannot block.
        match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(false),
            _ => return Err(last_error("waiting for a sealed command member")),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_departed_member_does_not_make_a_successful_snapshot_incomplete() {
        // Observed on a sealed Windows Job: assigned=1, returned=0 while its retained root is signaled.
        let mut list = ProcessList::empty();
        list.assigned = 1;
        assert!(
            list.listed_ids()
                .expect("the terminated member has left")
                .is_empty()
        );
        list.assigned = 2;
        list.count = 1;
        list.ids[0] = 42;
        assert_eq!(
            list.listed_ids().expect("the remaining member is retained"),
            &[42]
        );
    }

    #[test]
    fn oversized_or_inconsistent_membership_cannot_authorize_cleanup() {
        let mut list = ProcessList::empty();
        list.assigned = u32::try_from(MAX_RETAINED_PROCESSES + 1).expect("test capacity fits");
        list.count = u32::try_from(MAX_RETAINED_PROCESSES).expect("test capacity fits");
        assert!(list.listed_ids().is_err());
        list.assigned = 0;
        list.count = 1;
        assert!(list.listed_ids().is_err());
    }
}
