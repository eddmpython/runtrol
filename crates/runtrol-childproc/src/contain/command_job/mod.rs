//! A short command owns a nested Job before its first instruction can execute.
//!
//! The Runtime's outer Job remains unchanged. A cancelled command keeps any caller lease until the
//! kernel reports that this Job has no active processes, including descendants with closed stdio.

use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
};

use crate::error::SpawnError;

mod members;

enum Completion {
    Retained(Vec<OwnedHandle>),
    Unproven(String),
}

pub(super) type Lease = Arc<dyn Send + Sync>;
const STOP_DEADLINE: Duration = Duration::from_secs(5);
const STOP_POLL: Duration = Duration::from_millis(5);

pub(super) struct CommandJob {
    handle: OwnedHandle,
    lease: Option<Lease>,
    failed_root: Option<tokio::process::Child>,
    completion: OnceLock<Completion>,
}

impl CommandJob {
    #[expect(
        unsafe_code,
        reason = "the Windows Job API has no safe wrapper; each pointer is scoped to its call"
    )]
    pub(super) fn new(lease: Option<Lease>) -> Result<Self, SpawnError> {
        // SAFETY: null attributes and name create a private, non-inheritable Job handle.
        let raw = unsafe { CreateJobObjectW(core::ptr::null(), core::ptr::null()) };
        let handle = owned(raw, "creating a command Job")?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the information class matches the initialized struct and the Job handle is owned.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                core::ptr::from_ref(&limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            )
        } == 0
        {
            return Err(last_error("setting command Job containment"));
        }
        Ok(Self {
            handle,
            lease,
            failed_root: None,
            completion: OnceLock::new(),
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
        if unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), process) } == 0 {
            return Err(last_error("assigning a suspended command to its Job"));
        }
        let thread = primary_thread(pid)?;
        // SAFETY: primary_thread opened and rechecked the sole thread of the retained process.
        let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
        if previous != 1 {
            return Err(failure(
                "resuming the contained command",
                format!("unexpected suspend count {previous}"),
            ));
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "querying and terminating this private owned Job requires the Windows API"
    )]
    pub(super) fn request_stop(&self) -> Result<(), SpawnError> {
        let completion =
            self.completion
                .get_or_init(|| match members::seal(self.handle.as_raw_handle()) {
                    Ok(handles) => Completion::Retained(handles),
                    Err(error) => Completion::Unproven(error.to_string()),
                });
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
        // SAFETY: only the command's private Job is targeted; the Runtime Job is never passed here.
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), 1) } == 0 {
            return Err(last_error("stopping a command Job"));
        }
        if let Completion::Unproven(error) = completion {
            return Err(failure(
                "retaining command cleanup ownership",
                error.clone(),
            ));
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "the kernel active-process count is the completion evidence for a Job"
    )]
    pub(super) fn is_empty(&self) -> Result<bool, SpawnError> {
        match self.completion.get() {
            None => return Ok(false),
            Some(Completion::Unproven(error)) => {
                return Err(failure("checking command cleanup", error.clone()));
            }
            Some(Completion::Retained(handles)) if !members::stopped(handles)? => return Ok(false),
            Some(Completion::Retained(_)) => {}
        }
        if !self.failed_root_stopped()? {
            return Ok(false);
        }
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: the output buffer and size match this information class and outlive the call.
        if unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                core::ptr::from_mut(&mut accounting).cast(),
                u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()).unwrap_or(0),
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error("checking command Job completion"));
        }
        Ok(accounting.ActiveProcesses == 0)
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
        self.completion = OnceLock::from(Completion::Retained(Vec::new()));
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

#[expect(
    unsafe_code,
    reason = "a successful Windows handle is converted once into an owning safe wrapper"
)]
fn owned(raw: HANDLE, doing: &'static str) -> Result<OwnedHandle, SpawnError> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Err(last_error(doing));
    }
    // SAFETY: the caller transfers the successful newly-opened handle exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn failure(doing: &'static str, detail: impl Into<String>) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: detail.into(),
    }
}

fn last_error(doing: &'static str) -> SpawnError {
    failure(doing, std::io::Error::last_os_error().to_string())
}

#[cfg(test)]
mod tests;
