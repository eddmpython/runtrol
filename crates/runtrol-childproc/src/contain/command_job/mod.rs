//! A short command owns a nested Job before its first instruction can execute.
//!
//! The Runtime's outer Job remains unchanged. A cancelled command keeps any caller lease until the
//! kernel reports that this Job has no active processes, including descendants with closed stdio.

use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_NO_MORE_FILES, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::Threading::{
    GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
};

use crate::error::SpawnError;

use super::job::{Job, failure, last_error, owned};

pub(super) type Lease = Arc<dyn Send + Sync>;
const STOP_DEADLINE: Duration = Duration::from_secs(5);
const STOP_POLL: Duration = Duration::from_millis(5);

pub(super) struct CommandJob {
    job: Job,
    lease: Option<Lease>,
    failed_root: Option<tokio::process::Child>,
}

impl CommandJob {
    #[cfg(test)]
    pub(super) fn new(lease: Option<Lease>) -> Result<Self, SpawnError> {
        Self::for_client(lease, None)
    }

    pub(super) fn for_client(
        lease: Option<Lease>,
        client: Option<&Arc<super::keeper::Client>>,
    ) -> Result<Self, SpawnError> {
        Ok(Self {
            job: match client {
                Some(client) => Job::kept(client, super::keeper::Kind::Command)?,
                None => Job::new(1)?,
            },
            lease,
            failed_root: None,
        })
    }

    #[expect(
        unsafe_code,
        reason = "assigning the retained suspended child handle is the atomic execution boundary"
    )]
    pub(super) fn assign_and_resume(
        &self,
        child: &tokio::process::Child,
    ) -> Result<(), SpawnError> {
        let process = child.raw_handle().ok_or_else(|| {
            failure(
                "assigning a command Job",
                "the suspended child handle disappeared",
            )
        })?;
        let pid = child.id().ok_or_else(|| {
            failure(
                "assigning a command Job",
                "the suspended child identity disappeared",
            )
        })?;
        // SAFETY: Tokio owns this still-suspended process handle throughout assignment and resume.
        // The private Job is empty, has no UI limits, and permits no breakaway from the Runtime Job.
        if unsafe { AssignProcessToJobObject(self.job.handle.as_raw_handle(), process) } == 0 {
            return Err(last_error("assigning a suspended command to its Job"));
        }
        self.job.admit(
            // SAFETY: the suspended child owns this exact handle through the admission ACK.
            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(process) },
        )?;
        let thread = primary_thread(pid)?;
        self.job.ready()?;
        resume_suspended_thread(&thread, "resuming the contained command")
    }

    #[expect(
        unsafe_code,
        reason = "querying and terminating this private owned Job requires the Windows API"
    )]
    pub(super) fn request_stop(&self) -> Result<(), SpawnError> {
        if let Some(child) = &self.failed_root
            && !self.failed_root_stopped()?
        {
            let root = child.raw_handle().ok_or_else(|| {
                failure(
                    "stopping a failed command root",
                    "the retained root handle disappeared",
                )
            })?;
            // SAFETY: this exact suspended root handle is owned by failed_root until termination proof.
            if unsafe { TerminateProcess(root, 1) } == 0 {
                return Err(last_error("stopping a failed suspended command"));
            }
        }
        self.job.request_stop()
    }

    pub(super) fn is_empty(&self) -> Result<bool, SpawnError> {
        Ok(self.failed_root_stopped()? && self.job.is_empty()?)
    }

    pub(super) async fn stop(&self) -> Result<(), SpawnError> {
        self.request_stop()?;
        let started = Instant::now();
        loop {
            if self.is_empty()? {
                return Ok(());
            }
            if started.elapsed() >= STOP_DEADLINE {
                return Err(failure(
                    "waiting for command Job completion",
                    "processes remain after the termination deadline",
                ));
            }
            tokio::time::sleep(STOP_POLL).await;
        }
    }

    pub(super) fn retire_failed(mut self, child: tokio::process::Child) {
        // Assignment itself can fail. An empty Job then proves nothing about the suspended root.
        self.failed_root = Some(child);
        // The sole root never completed its suspended launch. Its retained handle is the whole proof.
        self.job.retain_failed_assignment();
        self.retire();
    }

    #[expect(
        unsafe_code,
        reason = "the retained failed root handle is the exact kernel termination evidence"
    )]
    fn failed_root_stopped(&self) -> Result<bool, SpawnError> {
        let Some(child) = &self.failed_root else {
            return Ok(true);
        };
        let root = child.raw_handle().ok_or_else(|| {
            failure(
                "checking a failed command root",
                "the retained root handle disappeared",
            )
        })?;
        // SAFETY: failed_root retains the process handle, and a zero timeout only queries its state.
        match unsafe { WaitForSingleObject(root, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(last_error("checking failed command root completion")),
        }
    }

    pub(super) fn retire(self) {
        if let Err(error) = self.request_stop() {
            eprintln!("runtrol: {error}");
        }
        if matches!(self.is_empty(), Ok(true)) {
            return;
        }
        let cleanup = Arc::new(self);
        let worker = Arc::clone(&cleanup);
        match std::thread::Builder::new()
            .name("command-cleanup".to_owned())
            .spawn(move || {
                let started = Instant::now();
                loop {
                    match worker.is_empty() {
                        Ok(true) => return,
                        Ok(false) if started.elapsed() < STOP_DEADLINE => {
                            std::thread::sleep(STOP_POLL);
                        }
                        outcome => {
                            eprintln!(
                                "runtrol: command cleanup could not prove completion: {outcome:?}"
                            );
                            // A failed completion proof cannot authorize reuse. The already-requested Job
                            // termination continues, while this bounded operation remains quarantined.
                            Self::quarantine_if_leased(worker);
                            return;
                        }
                    }
                }
            }) {
            Ok(handle) => drop(handle),
            Err(error) => {
                eprintln!("runtrol: could not start command cleanup: {error}");
                Self::quarantine_if_leased(cleanup);
            }
        }
    }

    fn quarantine_if_leased(owner: Arc<Self>) {
        if owner.lease.is_some() {
            eprintln!("runtrol: retaining the unproven command resource lease until Runtime exit");
            // The Job has already received termination. Releasing the caller's bounded lock here
            // would falsely authorize reuse. Runtime exit releases it for exact generation recovery.
            #[expect(
                clippy::disallowed_methods,
                reason = "retains a failed-closed resource lock and its terminated Job until Runtime exit; it does not abandon a running child"
            )]
            std::mem::forget(owner);
        }
    }
}

/// Execute a primary thread retained from suspended creation exactly once.
#[expect(
    unsafe_code,
    reason = "the owned primary-thread handle identifies the execution boundary"
)]
pub(crate) fn resume_suspended_thread(
    thread: &OwnedHandle,
    doing: &'static str,
) -> Result<(), SpawnError> {
    // SAFETY: the caller owns this primary-thread handle throughout the call. Both callers obtain it
    // from a process created suspended and surrender its resume authority after this single attempt.
    let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
    if previous != 1 {
        return Err(failure(
            doing,
            format!("unexpected suspend count {previous}"),
        ));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "thread enumeration retains handles and rechecks process ownership before resume"
)]
fn primary_thread(pid: u32) -> Result<OwnedHandle, SpawnError> {
    let snapshot = owned(
        // SAFETY: the snapshot flags and zero PID request the documented system thread inventory.
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) },
        "listing the suspended command's threads",
    )?;
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(0),
        ..Default::default()
    };
    let mut candidate = None;
    // SAFETY: entry is correctly sized and initialized; snapshot remains open for enumeration.
    let mut next = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) };
    while next != 0 {
        if entry.th32OwnerProcessID == pid {
            let thread = owned(
                // SAFETY: OpenThread checks the snapshot TID; the ownership check below catches TID reuse.
                unsafe {
                    OpenThread(
                        THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                        0,
                        entry.th32ThreadID,
                    )
                },
                "opening the suspended command thread",
            )?;
            // SAFETY: the opened thread handle is retained through this call and the later resume.
            if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != pid || candidate.is_some()
            {
                return Err(failure(
                    "checking the suspended command thread",
                    "thread ownership or cardinality changed",
                ));
            }
            candidate = Some(thread);
        }
        // SAFETY: same valid snapshot and output buffer as Thread32First.
        next = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) };
    }
    // SAFETY: this immediately observes the terminating thread enumeration call on the same thread.
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(last_error("enumerating the suspended command threads"));
    }
    candidate.ok_or_else(|| {
        failure(
            "finding the suspended command thread",
            "no owned thread was present",
        )
    })
}

#[cfg(test)]
mod tests;
