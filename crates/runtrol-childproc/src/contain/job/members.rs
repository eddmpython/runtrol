//! Seal owned membership before retaining the kernel objects that cleanup must wait for.

use std::os::windows::io::{AsHandle as _, AsRawHandle as _, OwnedHandle};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use super::{failure, last_error, owned};
use crate::SpawnError;

// An excessive tree is terminated but cannot release a caller lease without complete ownership proof.
const MAX_RETAINED_PROCESSES: usize = 4096;

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
                "retaining owned Job members",
                format!(
                    "invalid bounded list: assigned {}, returned {}, capacity {MAX_RETAINED_PROCESSES}",
                    self.assigned, self.count
                ),
            ));
        }
        let count = usize::try_from(self.count)
            .map_err(|_| failure("retaining owned Job members", "invalid member count"))?;
        self.ids.get(..count).ok_or_else(|| {
            failure(
                "retaining owned Job members",
                "the member count exceeds its buffer",
            )
        })
    }
}

#[expect(
    unsafe_code,
    reason = "membership sealing and retained process handles are Windows kernel operations"
)]
pub(super) fn seal(job: HANDLE) -> Result<Vec<Member>, SpawnError> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // With the flag present, zero rejects every subsequent assignment, including inherited children.
    // The real Windows populated-Job test also proves existing members stay available for exact waits.
    limits.BasicLimitInformation.ActiveProcessLimit = 0;
    // SAFETY: job is borrowed from the owning Job; the class and initialized buffer agree.
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            core::ptr::from_ref(&limits).cast(),
            u32::try_from(size_of_val(&limits)).unwrap_or(0),
        )
    } == 0
    {
        return Err(last_error("sealing owned Job membership"));
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
        return Err(last_error("retaining owned Job members"));
    }
    let ids = list.listed_ids()?;
    ids.iter()
        .map(|&id| {
            let pid = u32::try_from(id)
                .map_err(|_| failure("retaining an owned Job member", "invalid process id"))?;
            Ok(Member {
                pid,
                process: OnceLock::new(),
                membership: OnceLock::new(),
                #[cfg(test)]
                fail_open: std::sync::atomic::AtomicBool::new(false),
            })
        })
        .collect()
}

/// The complete sealed PID list is retained even when one individual process query fails.
/// Successful opens stay pinned across retries; they can never be replaced by a reused PID.
#[derive(Debug)]
pub(super) struct Member {
    pid: u32,
    process: OnceLock<Option<OwnedHandle>>,
    membership: OnceLock<bool>,
    #[cfg(test)]
    fail_open: std::sync::atomic::AtomicBool,
}

impl Member {
    #[expect(
        unsafe_code,
        reason = "retaining a sealed PID and checking its exact Job handle requires Windows"
    )]
    pub(super) fn resolve(&self, job: HANDLE) -> Result<(), SpawnError> {
        if self.membership.get().is_some() {
            return Ok(());
        }
        if self.process.get().is_none() {
            #[cfg(test)]
            if self
                .fail_open
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                return Err(failure(
                    "opening a sealed Job member",
                    "injected transient query failure",
                ));
            }
            // SAFETY: query and wait rights only, and the PID comes from the complete sealed list.
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    self.pid,
                )
            };
            let process = if raw.is_null() {
                let error = std::io::Error::last_os_error();
                if !error
                    .raw_os_error()
                    .is_some_and(|code| u32::try_from(code) == Ok(ERROR_INVALID_PARAMETER))
                {
                    return Err(failure("opening a sealed Job member", error.to_string()));
                }
                // ERROR_INVALID_PARAMETER means the process object no longer exists, never access denial.
                None
            } else {
                Some(owned(raw, "retaining a sealed Job member")?)
            };
            // A racing successful resolver may already own the same exact process. Preserve that
            // first handle and close only the redundant newly-opened one returned by set.
            drop(self.process.set(process));
        }
        let Some(process) = self.process.get() else {
            return Err(failure(
                "retaining a sealed Job member",
                "process resolution did not publish",
            ));
        };
        let member = match process {
            None => false,
            Some(process) => {
                let mut member = 0;
                // SAFETY: the first opened process and the Job remain retained. A failed query leaves
                // the exact handle owned for a retry rather than resolving a later process by PID.
                if unsafe { IsProcessInJob(process.as_raw_handle(), job, &raw mut member) } == 0 {
                    return Err(last_error("validating a sealed Job member"));
                }
                member != 0
            }
        };
        // A process cannot leave a Job and continue executing elsewhere. This classification applies
        // to the fixed retained handle; parallel successful observations share the first answer.
        self.membership.get_or_init(|| member);
        Ok(())
    }

    fn handle(&self) -> Result<Option<&OwnedHandle>, SpawnError> {
        match self.membership.get() {
            Some(false) => Ok(None),
            Some(true) => self
                .process
                .get()
                .and_then(Option::as_ref)
                .map(Some)
                .ok_or_else(|| {
                    failure(
                        "waiting for a sealed Job member",
                        "the exact member handle is unavailable",
                    )
                }),
            None => Err(failure(
                "waiting for a sealed Job member",
                "member identity is still unproven",
            )),
        }
    }

    pub(super) async fn wait(&self) -> Result<(), SpawnError> {
        if let Some(process) = self.handle()? {
            super::wait::signaled(process.as_handle()).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn fail_open_once(&self) {
        self.fail_open
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn retained(&self) -> bool {
        self.process.get().is_some_and(Option::is_some)
    }
}

pub(super) fn resolve(members: &[Member], job: HANDLE) -> Result<(), SpawnError> {
    let mut failure = None;
    for member in members {
        if let Err(error) = member.resolve(job) {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

pub(super) fn stopped(members: &[Member]) -> Result<bool, SpawnError> {
    for member in members {
        if let Some(process) = member.handle()?
            && !super::wait::is_signaled(process.as_handle())?
        {
            return Ok(false);
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
