//! Exact private Windows objects for the keeper bootstrap and scope events.

#![expect(
    unsafe_code,
    reason = "this module owns the documented Win32 handle and process operations"
)]

use std::fs::File;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsHandle as _, AsRawHandle as _, BorrowedHandle, OwnedHandle};

use runtrol_provider::ProcessIdentity;
use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateProcessW, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, GetProcessId, OpenProcess, PROCESS_INFORMATION, STARTUPINFOEXW, SetEvent,
};

use crate::SpawnError;
use crate::contain::job::{failure, last_error, owned};
use crate::process_attributes::ProcessAttributes;

pub(super) const SYNCHRONIZE: u32 = 0x0010_0000;
pub(super) const EVENT_MODIFY: u32 = 0x0002;
pub(super) const JOB_ASSIGN_QUERY: u32 = 0x0001 | 0x0004;
pub(super) const PROCESS_QUERY_WAIT: u32 = 0x1000 | SYNCHRONIZE;

#[derive(Debug)]
pub(super) struct Process {
    pub(super) handle: OwnedHandle,
    pub(super) identity: ProcessIdentity,
}

impl Process {
    pub(super) fn open(expected: ProcessIdentity) -> Result<Self, SpawnError> {
        let handle = owned(
            // SAFETY: read-only observation opens an exact incarnation and rechecks it on this handle.
            unsafe { OpenProcess(PROCESS_QUERY_WAIT, 0, expected.pid()) },
            "retaining exact completion process",
        )?;
        let identity = identity(handle.as_handle())?;
        if identity != expected {
            return Err(failure(
                "retaining exact completion process",
                "the process incarnation changed",
            ));
        }
        Ok(Self { handle, identity })
    }

    pub(super) fn exit_code(&self) -> Result<u32, SpawnError> {
        let mut code = 0;
        // SAFETY: the owner waits this exact handle before requesting its final exit status.
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &raw mut code) } == 0 {
            return Err(last_error("reading keeper completion status"));
        }
        Ok(code)
    }
    pub(super) fn received(raw: usize) -> Result<Self, SpawnError> {
        let handle = super::job_handle(raw)?;
        let identity = identity(handle.as_handle())?;
        Ok(Self { handle, identity })
    }
}

pub(super) fn identity(handle: BorrowedHandle<'_>) -> Result<ProcessIdentity, SpawnError> {
    // SAFETY: the exact process handle stays owned during both structural identity calls.
    let pid = unsafe { GetProcessId(handle.as_raw_handle()) };
    if pid == 0 {
        return Err(last_error("identifying the private process"));
    }
    let start = crate::process_tree::windows_start_from_handle(handle)
        .map_err(|error| failure("identifying the private process", error.to_string()))?;
    ProcessIdentity::new(pid, start).ok_or_else(|| {
        failure(
            "identifying the private process",
            "the process incarnation is absent",
        )
    })
}

pub(super) fn current() -> Result<Process, SpawnError> {
    let mut raw = core::ptr::null_mut();
    // SAFETY: a current-process pseudo-handle is converted into one real owned handle. It is not inherited.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentProcess(),
            GetCurrentProcess(),
            &raw mut raw,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(last_error("retaining the current process identity"));
    }
    let handle = owned(raw, "retaining the current process")?;
    let identity = identity(handle.as_handle())?;
    Ok(Process { handle, identity })
}

pub(super) fn event() -> Result<OwnedHandle, SpawnError> {
    owned(
        // SAFETY: one unnamed non-inheritable manual-reset event, initially nonsignaled.
        unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) },
        "creating private scope event",
    )
}

pub(super) fn signal(event: BorrowedHandle<'_>) -> Result<(), SpawnError> {
    // SAFETY: the caller owns an event handle with modify permission for this exact scope.
    if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
        return Err(last_error("requesting owned scope stop"));
    }
    Ok(())
}

pub(super) fn member(
    process: BorrowedHandle<'_>,
    job: BorrowedHandle<'_>,
) -> Result<bool, SpawnError> {
    let mut inside = 0;
    // SAFETY: both exact kernel objects remain borrowed through this membership query.
    if unsafe {
        IsProcessInJob(
            process.as_raw_handle(),
            job.as_raw_handle(),
            &raw mut inside,
        )
    } == 0
    {
        return Err(last_error("verifying private keeper membership"));
    }
    Ok(inside != 0)
}

pub(super) fn copy(
    handle: BorrowedHandle<'_>,
    target: BorrowedHandle<'_>,
    access: u32,
) -> Result<usize, SpawnError> {
    let mut copied = core::ptr::null_mut();
    // SAFETY: the authenticated peer receives one reference to this exact object with reduced access.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            target.as_raw_handle(),
            &raw mut copied,
            access,
            0,
            0,
        )
    } == 0
    {
        return Err(last_error("passing a private scope handle"));
    }
    Ok(copied as usize)
}

fn inherited(handle: BorrowedHandle<'_>) -> Result<OwnedHandle, SpawnError> {
    let mut raw = core::ptr::null_mut();
    // SAFETY: only the explicit bootstrap handle list inherits this real, freshly duplicated reference.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &raw mut raw,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(last_error("preparing private bootstrap inheritance"));
    }
    owned(raw, "retaining private bootstrap inheritance")
}

fn pipe() -> Result<(OwnedHandle, OwnedHandle), SpawnError> {
    let mut read = core::ptr::null_mut();
    let mut write = core::ptr::null_mut();
    // SAFETY: anonymous non-inheritable pipe ends are produced with the fixed short control buffer.
    if unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 4096) } == 0 {
        return Err(last_error("creating private keeper control"));
    }
    Ok((
        owned(read, "retaining private control read")?,
        owned(write, "retaining private control write")?,
    ))
}

pub(super) struct Started {
    pub(super) keeper: Process,
    pub(super) input: File,
    pub(super) output: File,
}

pub(super) fn start(parent: &Process) -> Result<Started, SpawnError> {
    let (request_read, request_write) = pipe()?;
    let (response_read, response_write) = pipe()?;
    let parent_copy = inherited(parent.handle.as_handle())?;
    let read_copy = inherited(request_read.as_handle())?;
    let write_copy = inherited(response_write.as_handle())?;
    let inherited = [
        parent_copy.as_raw_handle(),
        read_copy.as_raw_handle(),
        write_copy.as_raw_handle(),
    ];
    let mut attributes = ProcessAttributes::for_handles(&inherited)?;
    let program = std::env::current_exe()
        .map_err(|error| failure("finding the keeper image", error.to_string()))?;
    // Every argument after the known executable is a fixed private word or an integer. Neither paths from
    // the client nor provider arguments are assembled into a Windows command line here.
    let suffix = format!(
        "\" {} {} {} {}",
        super::bootstrap::ARGUMENT,
        parent_copy.as_raw_handle() as usize,
        read_copy.as_raw_handle() as usize,
        write_copy.as_raw_handle() as usize
    );
    let mut line: Vec<_> = [u16::from(b'"')]
        .into_iter()
        .chain(program.as_os_str().encode_wide())
        .chain(suffix.encode_utf16())
        .chain([0])
        .collect();
    let program: Vec<_> = program.as_os_str().encode_wide().chain([0]).collect();
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>()).unwrap_or(0);
    startup.lpAttributeList = attributes.as_ptr();
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: the explicit inheritance list contains the only three bootstrap objects. Their values and
    // the extended startup allocation remain owned through process creation. This precedes the outer Job.
    if unsafe {
        CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | 0x0800_0000,
            core::ptr::null(),
            core::ptr::null(),
            &raw const startup.StartupInfo,
            &raw mut process,
        )
    } == 0
    {
        return Err(last_error("starting the independent Job keeper"));
    }
    let handle = owned(process.hProcess, "retaining the keeper process")?;
    drop(owned(
        process.hThread,
        "retaining the keeper primary thread",
    )?);
    let identity = identity(handle.as_handle())?;
    drop((
        attributes,
        parent_copy,
        read_copy,
        write_copy,
        request_read,
        response_write,
    ));
    Ok(Started {
        keeper: Process { handle, identity },
        input: File::from(request_write),
        output: File::from(response_read),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_observer_cannot_signal_the_exact_event() {
        let producer = event().unwrap();
        let process = current().unwrap();
        let copied = copy(
            producer.as_handle(),
            process.handle.as_handle(),
            SYNCHRONIZE,
        )
        .unwrap();
        let observer = super::super::job_handle(copied).unwrap();
        assert!(signal(observer.as_handle()).is_err());
        assert!(!crate::contain::job::wait::is_signaled(producer.as_handle()).unwrap());
        signal(producer.as_handle()).unwrap();
        assert!(crate::contain::job::wait::is_signaled(observer.as_handle()).unwrap());
    }
}
