//! One private nested Job owns membership and exact completion for commands and terminals.

use std::os::windows::io::{AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};

use crate::SpawnError;

mod members;
mod scope;
pub(crate) mod wait;

pub use scope::ProcessScope;

#[derive(Debug)]
enum Completion {
    Retained(Vec<members::Member>),
    Unproven(String),
}

#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) handle: OwnedHandle,
    completion: OnceLock<Completion>,
    stopping: AtomicBool,
    termination_code: u32,
    keeper: Option<super::keeper::Scope>,
}

impl Job {
    #[expect(
        unsafe_code,
        reason = "private Job creation and limits are kernel operations"
    )]
    pub(crate) fn new(termination_code: u32) -> Result<Self, SpawnError> {
        let handle = owned(
            // SAFETY: null attributes and name create a private non-inheritable Job.
            unsafe { CreateJobObjectW(core::ptr::null(), core::ptr::null()) },
            "creating an owned Job",
        )?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the initialized buffer matches the class and the owned handle stays live.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                core::ptr::from_ref(&limits).cast(),
                u32::try_from(size_of_val(&limits)).unwrap_or(0),
            )
        } == 0
        {
            return Err(last_error("setting owned Job containment"));
        }
        Ok(Self {
            handle,
            completion: OnceLock::new(),
            stopping: AtomicBool::new(false),
            termination_code,
            keeper: None,
        })
    }

    pub(crate) fn kept(
        client: &Arc<super::keeper::Client>,
        kind: super::keeper::Kind,
    ) -> Result<Self, SpawnError> {
        let (handle, scope) = client.create(kind)?;
        Ok(Self {
            handle,
            completion: OnceLock::new(),
            stopping: AtomicBool::new(false),
            termination_code: super::TERMINATED_BY_RUNTROL,
            keeper: Some(scope),
        })
    }

    pub(crate) fn admit(&self, root: BorrowedHandle<'_>) -> Result<(), SpawnError> {
        if let Some(scope) = &self.keeper {
            scope.admit(root)?;
        }
        Ok(())
    }

    pub(crate) fn ready(&self) -> Result<(), SpawnError> {
        if let Some(scope) = &self.keeper {
            return scope.ready();
        }
        if self.is_sealed() {
            return Err(failure(
                "executing a prepared process",
                "its Job is stopping",
            ));
        }
        Ok(())
    }

    pub(crate) async fn wait_root(&self, root: BorrowedHandle<'_>) -> Result<(), SpawnError> {
        if let Some(scope) = &self.keeper {
            scope.wait_root(root).await
        } else {
            wait::signaled(root).await
        }
    }

    /// Only a never-executed command whose root is separately retained may use this proof.
    pub(crate) fn retain_failed_assignment(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.completion = OnceLock::from(Completion::Retained(Vec::new()));
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
            || self
                .keeper
                .as_ref()
                .is_some_and(|scope| scope.ready().is_err())
    }

    #[expect(
        unsafe_code,
        reason = "sealing and terminating this private Job is the ownership boundary"
    )]
    pub(crate) fn request_stop(&self) -> Result<(), SpawnError> {
        // Membership authority ends when stopping starts, including during the bounded capture.
        self.stopping.store(true, Ordering::Release);
        if let Some(scope) = &self.keeper {
            return scope.request_stop();
        }
        let completion =
            self.completion
                .get_or_init(|| match members::seal(self.handle.as_raw_handle()) {
                    Ok(handles) => Completion::Retained(handles),
                    Err(error) => Completion::Unproven(error.to_string()),
                });
        let resolved = match completion {
            Completion::Retained(members) => members::resolve(members, self.handle.as_raw_handle()),
            Completion::Unproven(error) => {
                Err(failure("retaining Job cleanup ownership", error.clone()))
            }
        };
        // SAFETY: only this owner's private Job is targeted, never its containing Runtime Job.
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), self.termination_code) } == 0 {
            return Err(last_error("stopping an owned Job"));
        }
        resolved
    }

    pub(crate) fn is_empty(&self) -> Result<bool, SpawnError> {
        if let Some(scope) = &self.keeper {
            return scope.is_empty();
        }
        match self.completion.get() {
            None => return Ok(false),
            Some(Completion::Unproven(error)) => {
                return Err(failure("checking Job cleanup", error.clone()));
            }
            Some(Completion::Retained(handles)) if !members::stopped(handles)? => return Ok(false),
            Some(Completion::Retained(_)) => {}
        }
        self.no_active_processes()
    }

    pub(crate) async fn stopped(self: &Arc<Self>) -> Result<(), SpawnError> {
        if let Some(scope) = &self.keeper {
            return scope.stopped().await;
        }
        let stopping = Arc::clone(self);
        // Enumerating and opening members is synchronous OS work. The worker retains this same Job
        // if its waiting future is cancelled, and never holds the Runtime's input admission locks.
        tokio::task::spawn_blocking(move || stopping.request_stop())
            .await
            .map_err(|error| failure("stopping the terminal Job", error.to_string()))??;
        let Some(Completion::Retained(handles)) = self.completion.get() else {
            return Err(failure(
                "waiting for owned Job completion",
                "membership is unproven",
            ));
        };
        for process in handles {
            process.wait().await?;
        }
        if !self.no_active_processes()? {
            return Err(failure(
                "waiting for owned Job completion",
                "the sealed Job still reports active processes",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_one_member_open(&self) {
        let members =
            members::seal(self.handle.as_raw_handle()).expect("the real bounded list is captured");
        members
            .first()
            .expect("the real Job has a member")
            .fail_open_once();
        assert!(self.completion.set(Completion::Retained(members)).is_ok());
    }

    #[cfg(test)]
    pub(crate) fn retained_members(&self) -> usize {
        match self.completion.get() {
            Some(Completion::Retained(members)) => {
                members.iter().filter(|member| member.retained()).count()
            }
            _ => 0,
        }
    }

    #[expect(
        unsafe_code,
        reason = "kernel accounting is required in addition to exact member waits"
    )]
    fn no_active_processes(&self) -> Result<bool, SpawnError> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: class, buffer and size agree and the Job remains owned throughout the call.
        if unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                core::ptr::from_mut(&mut accounting).cast(),
                u32::try_from(size_of_val(&accounting)).unwrap_or(0),
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error("checking owned Job completion"));
        }
        Ok(accounting.ActiveProcesses == 0)
    }
}

#[expect(
    unsafe_code,
    reason = "a successful kernel handle is transferred into one owning wrapper"
)]
pub(crate) fn owned(raw: HANDLE, doing: &'static str) -> Result<OwnedHandle, SpawnError> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Err(last_error(doing));
    }
    // SAFETY: the caller transfers this newly opened handle exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

pub(crate) fn failure(doing: &'static str, detail: impl Into<String>) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: detail.into(),
    }
}

pub(crate) fn last_error(doing: &'static str) -> SpawnError {
    failure(doing, std::io::Error::last_os_error().to_string())
}
