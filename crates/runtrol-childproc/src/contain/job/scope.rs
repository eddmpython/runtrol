//! Read-only membership in the exact terminal's nested Job.

use std::os::windows::io::{AsHandle as _, AsRawHandle as _};
use std::sync::Arc;

use runtrol_provider::ProcessIdentity;
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use super::{Job, failure, last_error, owned, wait};
use crate::SpawnError;

/// Read-only membership authority for one terminal and its non-breakaway descendants.
///
/// Clones retain the same kernel Job. They cannot add members, resume or stop a process.
#[derive(Clone, Debug)]
pub struct ProcessScope(Arc<Job>);

impl ProcessScope {
    pub(crate) fn new(job: Arc<Job>) -> Self {
        Self(job)
    }

    /// Check birth identity and membership using the same retained process handle.
    ///
    /// A stopping scope and an ended or reused process are never admitted.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when the kernel cannot prove identity or membership.
    #[expect(
        unsafe_code,
        reason = "exact process identity and Job membership are kernel facts"
    )]
    pub fn contains(&self, candidate: ProcessIdentity) -> Result<bool, SpawnError> {
        if self.0.is_sealed() {
            return Ok(false);
        }
        let handle = owned(
            // SAFETY: query and synchronization rights only; this call never mutates a process.
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    candidate.pid(),
                )
            },
            "opening a terminal member",
        )?;
        let started = crate::process_tree::windows_start_from_handle(handle.as_handle())
            .map_err(|error| failure("checking terminal member identity", error.to_string()))?;
        if started != candidate.started() || wait::is_signaled(handle.as_handle())? {
            return Ok(false);
        }
        let mut member = 0;
        // SAFETY: both kernel handles remain owned and the output is writable for this call.
        if unsafe {
            IsProcessInJob(
                handle.as_raw_handle(),
                self.0.handle.as_raw_handle(),
                &raw mut member,
            )
        } == 0
        {
            return Err(last_error("checking terminal Job membership"));
        }
        Ok(member != 0 && !self.0.is_sealed())
    }
}
