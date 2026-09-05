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
    let mut list = Box::<ProcessList>::new_uninit();
    // SAFETY: this allocation has space for one aligned ProcessList; every field is an integer,
    // so zero initializes the complete buffer directly on the heap without a large stack temporary.
    unsafe { list.as_mut_ptr().write_bytes(0, 1) };
    // SAFETY: the complete ProcessList was initialized above, including its variable-list capacity.
    let mut list = unsafe { list.assume_init() };
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
    if list.count != list.assigned {
        return Err(failure(
            "retaining command Job members",
            "the bounded list is incomplete",
        ));
    }
    let count = usize::try_from(list.count)
        .map_err(|_| failure("retaining command Job members", "invalid member count"))?;
    let ids = list.ids.get(..count).ok_or_else(|| {
        failure(
            "retaining command Job members",
            "the member count exceeds its buffer",
        )
    })?;
    let mut handles = Vec::with_capacity(count);
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
