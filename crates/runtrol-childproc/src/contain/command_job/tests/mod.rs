//! Real Windows children prove command containment independently of their stdio lifetime.

use std::fs::{File, OpenOptions};
use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use runtrol_provider::{AbsPath, ProcessIdentity};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, GetCurrentProcess, OpenProcess, PROCESS_ALL_ACCESS, SuspendThread,
    TerminateProcess, WaitForSingleObject,
};

use super::{CommandJob, Lease, owned, primary_thread};
use crate::{
    Containment, SpawnError, capture_in, matches_process_start, process_identity, resolve,
};

const HELPER_PREFIX: &str = "contain::command_job::tests::";
const HELPER_LIMIT: Duration = Duration::from_secs(30);
const OBSERVE_LIMIT: Duration = Duration::from_secs(10);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
#[expect(
    unsafe_code,
    reason = "the fixture seals only its own private Job before testing new membership"
)]
async fn a_sealed_job_refuses_new_members_without_ending_existing_ones() {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    let mut fixture = Fixture::new();
    let job = CommandJob::new(None).expect("create a private Job");
    let mut children = Vec::new();
    for _ in 0..2 {
        let child = suspended_child(&fixture.root);
        fixture
            .known
            .push(process_identity(child.id().expect("child PID")).expect("child identity"));
        assert_ne!(
            // SAFETY: both handles belong to this fixture; the child has not executed an instruction.
            unsafe {
                AssignProcessToJobObject(
                    job.handle.as_raw_handle(),
                    child.raw_handle().expect("child handle"),
                )
            },
            0
        );
        children.push(child);
    }
    fixture.observe().expect("retain both suspended children");
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    limits.BasicLimitInformation.ActiveProcessLimit = 0;
    // SAFETY: the initialized limit buffer matches the class and targets this private Job only.
    let sealed = unsafe {
        SetInformationJobObject(
            job.handle.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            core::ptr::from_ref(&limits).cast(),
            u32::try_from(size_of_val(&limits)).expect("limit size"),
        )
    };
    assert_ne!(
        sealed,
        0,
        "seal a populated Job: {}",
        std::io::Error::last_os_error()
    );
    assert!(
        fixture.observed.iter().all(|(_, handle)| !signaled(handle)),
        "sealing must leave the captured members available for exact waits"
    );
    let mut refused = suspended_child(&fixture.root);
    let refused_handle = duplicate(
        refused.raw_handle().expect("new child handle"),
        0,
        DUPLICATE_SAME_ACCESS,
    );
    // SAFETY: both handles are retained fixture objects; no external process is addressed.
    let admitted = unsafe {
        AssignProcessToJobObject(
            job.handle.as_raw_handle(),
            refused.raw_handle().expect("new child handle"),
        )
    };
    let refusal = std::io::Error::last_os_error();
    stop_handle(&refused_handle).expect("reap the attempted child even if the assertion fails");
    refused.wait().await.expect("reap refused child");
    job.request_stop().expect("terminate the owned Job");
    for child in &mut children {
        child.wait().await.expect("reap contained child");
    }
    assert_eq!(
        admitted, 0,
        "a sealed Job admitted another process: {refusal}"
    );
}

#[tokio::test]
async fn a_command_job_nests_without_terminating_its_outer_owner() {
    let mut fixture = Fixture::new();
    let output = run_helper(&mut fixture, "nested_owner", OBSERVE_LIMIT).await;
    let proved = fixture.root.join("nested-proved").is_file();
    let stopped = fixture.recorded_processes_stopped();
    drop(fixture);
    assert!(output.expect("the nested owner completed").succeeded());
    assert!(
        proved,
        "the child belonged to both Jobs and its outer owner survived"
    );
    assert!(stopped, "nested children ended before fixture cleanup");
}

#[tokio::test]
async fn a_capture_timeout_stops_a_real_descendant() {
    let mut fixture = Fixture::new();
    let result = run_helper(&mut fixture, "held_root", Duration::from_secs(8)).await;
    let ready = fixture.root.join("root-ready").is_file();
    let stopped = fixture.recorded_processes_stopped();
    drop(fixture);
    assert!(
        ready,
        "both real processes started before the command deadline"
    );
    assert!(
        matches!(result, Err(SpawnError::Timeout { .. })),
        "{result:?}"
    );
    assert!(
        stopped,
        "timeout completion left a root or descendant alive"
    );
}

#[tokio::test]
async fn a_successful_root_cannot_leave_a_descendant_with_closed_stdio() {
    let mut fixture = Fixture::new();
    let result = run_helper(&mut fixture, "exiting_root", OBSERVE_LIMIT).await;
    let ready = fixture.root.join("root-ready").is_file();
    let stopped = fixture.recorded_processes_stopped();
    if !stopped {
        eprintln!(
            "capture returned with native process states {:?}",
            fixture
                .observed
                .iter()
                .map(|(id, handle)| (id.pid(), wait_state(handle)))
                .collect::<Vec<_>>()
        );
    }
    drop(fixture);
    assert!(result.expect("the root completed normally").succeeded());
    assert!(ready, "the root observed its descendant before exiting");
    assert!(
        stopped,
        "natural root completion left the detached stdio child alive"
    );
}

#[tokio::test]
async fn failed_assignment_and_resume_retain_the_lease_until_the_root_stops() {
    for fail_assignment in [true, false] {
        let mut fixture = Fixture::new();
        let child = suspended_child(&fixture.root);
        let pid = child.id().expect("the suspended child has an identity");
        fixture
            .known
            .push(process_identity(pid).expect("the retained child is inspectable"));
        fixture
            .observe()
            .expect("retain the suspended root before termination");
        let root = duplicate(
            child.raw_handle().expect("the child retains its handle"),
            0,
            DUPLICATE_SAME_ACCESS,
        );
        let release_state = Arc::new(AtomicU8::new(0));
        let lease: Lease = Arc::new(ObservedLease {
            _lock: fixture.lock(),
            root: duplicate(root.as_raw_handle(), 0, DUPLICATE_SAME_ACCESS),
            release_state: Arc::clone(&release_state),
        });
        let weak = Arc::downgrade(&lease);
        let mut job = CommandJob::new(Some(lease)).expect("the command Job is created");
        if fail_assignment {
            // The same valid Job still permits query and termination, but assignment must fail.
            job.handle = duplicate(
                job.handle.as_raw_handle(),
                JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE,
                0,
            );
        } else {
            let thread = primary_thread(pid).expect("the suspended primary thread exists");
            add_suspend(&thread);
        }
        let failure = job.assign_and_resume(&child);
        job.retire_failed(child);
        let deadline = Instant::now() + OBSERVE_LIMIT;
        while (weak.upgrade().is_some() || release_state.load(Ordering::SeqCst) == 0)
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let lease_released = weak.upgrade().is_none();
        let root_stopped = signaled(&root);
        let never_ran = !fixture.root.join("leaf.pid").exists();
        drop(fixture);
        let expected = if fail_assignment {
            "assigning a suspended command to its Job"
        } else {
            "resuming the contained command"
        };
        assert!(
            matches!(&failure, Err(SpawnError::Containment { doing, .. }) if *doing == expected),
            "the real Windows fault must reach {expected}: {failure:?}"
        );
        assert!(never_ran, "failed launch executed the helper body");
        assert!(root_stopped, "failed launch left its retained root alive");
        assert!(
            lease_released,
            "confirmed root termination must release the caller lease"
        );
        assert_eq!(
            release_state.load(Ordering::SeqCst),
            1,
            "the resource lease was released before its root had stopped"
        );
    }
}

async fn run_helper(
    fixture: &mut Fixture,
    name: &str,
    within: Duration,
) -> Result<crate::Output, SpawnError> {
    let executable = std::env::current_exe().expect("the test executable exists");
    let program = resolve(executable.to_str().expect("the executable path is UTF-8"))?;
    let directory =
        AbsPath::canonicalize(fixture.root.to_str().expect("the fixture path is UTF-8"))
            .expect("the fixture directory exists");
    let arguments = helper_args(name);
    let containment = Containment::without_any();
    let mut running = Box::pin(capture_in(
        &program,
        &arguments,
        &directory,
        within,
        &containment,
    ));
    let mut observed = false;
    loop {
        tokio::select! {
            result = &mut running => return result,
            () = tokio::time::sleep(Duration::from_millis(5)), if !observed => {
                if fixture.root.join("root-ready").is_file() {
                    fixture.observe().expect("retain exact process handles before stopping the command");
                    std::fs::write(fixture.root.join("observer-ready"), b"observed").expect("acknowledge retained handles");
                    observed = true;
                }
            }
        }
    }
}

fn helper_args(name: &str) -> Vec<String> {
    vec![
        "--exact".to_owned(),
        format!("{HELPER_PREFIX}{name}"),
        "--ignored".to_owned(),
        "--nocapture".to_owned(),
        "--test-threads=1".to_owned(),
    ]
}

fn helper_command(root: &Path, name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("the test executable exists"));
    command
        .args(helper_args(name))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::hide_console_window(&mut command);
    command
}

fn suspended_child(root: &Path) -> tokio::process::Child {
    let mut command = tokio::process::Command::from(helper_command(root, "leaf"));
    crate::console_window::hide_console_window_with_flags(command.as_std_mut(), CREATE_SUSPENDED);
    command
        .kill_on_drop(true)
        .spawn()
        .expect("the test child starts suspended")
}

#[test]
#[ignore = "entry point for a contained descendant with no inherited stdio"]
fn leaf() {
    let root = helper_root();
    record_process(&root, "leaf");
    wait_for_release(&root);
}

#[test]
#[ignore = "entry point for a command that holds a real descendant until timeout"]
fn held_root() {
    run_root(false);
}

#[test]
#[ignore = "entry point for a root that exits while its descendant remains live"]
fn exiting_root() {
    run_root(true);
}

fn run_root(exit_first: bool) {
    let root = helper_root();
    record_process(&root, "root");
    let mut child = helper_command(&root, "leaf")
        .spawn()
        .expect("the descendant starts");
    wait_for_file(&root.join("leaf.pid"));
    std::fs::write(root.join("root-ready"), b"ready")
        .expect("the root records descendant readiness");
    wait_for_file(&root.join("observer-ready"));
    if exit_first {
        // Standard Child drop deliberately leaves this process to the capture Job's completion path.
        drop(child);
    } else {
        wait_for_release(&root);
        child.wait().expect("the released descendant is reaped");
    }
}

#[tokio::test]
#[ignore = "the outer Job must be established only inside this disposable helper"]
async fn nested_owner() {
    let root = helper_root();
    record_process(&root, "root");
    let outer = Containment::establish().expect("the disposable owner establishes its outer Job");
    let job = CommandJob::new(None).expect("the nested command Job is created");
    let mut child = suspended_child(&root);
    job.assign_and_resume(&child)
        .expect("the suspended child joins and resumes in the nested Job");
    wait_for_file(&root.join("leaf.pid"));
    let identity =
        process_identity(child.id().expect("the child has a PID")).expect("the child is live");
    let outer_member = outer
        .contains(identity, identity)
        .expect("outer membership is inspectable");
    let inner_member = in_job(
        &job,
        child.raw_handle().expect("the child retains its handle"),
    );
    std::fs::write(root.join("root-ready"), b"ready").expect("publish nested process readiness");
    wait_for_file(&root.join("observer-ready"));
    job.stop().await.expect("only the inner command Job stops");
    child.wait().await.expect("the nested root is reaped");
    assert!(
        outer_member && inner_member,
        "the child belongs to both Job boundaries"
    );
    assert!(job.is_empty().expect("the nested Job can be inspected"));
    std::fs::write(root.join("nested-proved"), b"outer owner survived")
        .expect("the live outer owner records success");
    // The process owns this handle until exit. Dropping a self-containing Job kills the test harness
    // before it reports the helper's success; the OS closes the handle when this helper exits.
    #[expect(
        clippy::disallowed_methods,
        reason = "this disposable helper's self-containing Job must remain owned until OS process teardown, after the test harness reports its result"
    )]
    std::mem::forget(outer);
}

fn helper_root() -> PathBuf {
    let root = std::env::current_dir().expect("the helper has a directory");
    assert!(
        root.join("fixture-owner").is_file(),
        "only an owned fixture can run helper code"
    );
    root
}

fn record_process(root: &Path, name: &str) {
    let identity =
        process_identity(std::process::id()).expect("the helper identity is inspectable");
    let temporary = root.join(format!("{name}.starting"));
    std::fs::write(
        &temporary,
        format!("{} {}", identity.pid(), identity.started()),
    )
    .expect("the helper writes its exact identity");
    std::fs::rename(temporary, root.join(format!("{name}.pid")))
        .expect("the complete identity is published atomically");
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + OBSERVE_LIMIT;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "helper readiness deadline: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_release(root: &Path) {
    let deadline = Instant::now() + HELPER_LIMIT;
    while !root.join("release").is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Fixture {
    root: PathBuf,
    base: PathBuf,
    marker: String,
    known: Vec<ProcessIdentity>,
    observed: Vec<(ProcessIdentity, OwnedHandle)>,
}

struct ObservedLease {
    _lock: File,
    root: OwnedHandle,
    release_state: Arc<AtomicU8>,
}

impl Drop for ObservedLease {
    fn drop(&mut self) {
        // Observe at the release boundary itself. A later poll could miss a short unsafe unlock.
        self.release_state
            .store(if signaled(&self.root) { 1 } else { 2 }, Ordering::SeqCst);
    }
}

impl Fixture {
    fn new() -> Self {
        let shared = PathBuf::from(
            std::env::var_os("LOCALAPPDATA").expect("Windows exposes local app data"),
        )
        .join("dev-workspace");
        let base = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || shared.clone(),
            |target| {
                PathBuf::from(target)
                    .parent()
                    .expect("the shared Cargo target has a parent")
                    .to_path_buf()
            },
        );
        assert!(
            base.is_absolute() && base.starts_with(&shared),
            "fixtures must stay inside the shared execution root"
        );
        std::fs::create_dir_all(&base).expect("the shared fixture parent exists");
        let base = base
            .canonicalize()
            .expect("the fixture parent is canonical");
        let marker = format!(
            "command-job-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        );
        let root = base.join(&marker);
        std::fs::create_dir(&root).expect("the task creates a unique fixture");
        std::fs::write(root.join("fixture-owner"), &marker)
            .expect("the fixture records its exact owner");
        Self {
            root,
            base,
            marker,
            known: Vec::new(),
            observed: Vec::new(),
        }
    }

    fn lock(&self) -> File {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("operation.lock"))
            .expect("the operation lock opens");
        lock.try_lock().expect("the operation lease is acquired");
        lock
    }

    fn identities(&self) -> Result<Vec<ProcessIdentity>, String> {
        let mut identities = self.known.clone();
        for name in ["root", "leaf"] {
            match std::fs::read_to_string(self.root.join(format!("{name}.pid"))) {
                Ok(text) => {
                    let (pid, started) = text
                        .split_once(' ')
                        .ok_or("the helper identity is incomplete")?;
                    let pid = pid
                        .parse()
                        .map_err(|error| format!("invalid helper PID: {error}"))?;
                    let started = started
                        .parse()
                        .map_err(|error| format!("invalid helper birth stamp: {error}"))?;
                    identities.push(
                        ProcessIdentity::new(pid, started)
                            .ok_or("invalid helper process identity")?,
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("cannot inspect an owned helper: {error}")),
            }
        }
        Ok(identities)
    }

    #[expect(
        unsafe_code,
        reason = "the fixture opens exact published live identities before granting permission to exit"
    )]
    fn observe(&mut self) -> Result<(), String> {
        for identity in self.identities()? {
            let handle = owned(
                // SAFETY: only identities from this fixture are opened and the birth stamp is rechecked.
                unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, identity.pid()) },
                "retaining an owned fixture process",
            )
            .map_err(|error| error.to_string())?;
            if process_identity(identity.pid()) != Some(identity) {
                return Err("the fixture process changed before observation".to_owned());
            }
            self.observed.push((identity, handle));
        }
        Ok(())
    }

    fn recorded_processes_stopped(&self) -> bool {
        !self.observed.is_empty() && self.observed.iter().all(|(_, handle)| signaled(handle))
    }

    fn cleanup(&mut self) -> Result<(), String> {
        std::fs::write(self.root.join("release"), b"release").map_err(|error| error.to_string())?;
        for identity in self.identities()? {
            if let Some((_, handle)) = self
                .observed
                .iter()
                .find(|(observed, _)| *observed == identity)
            {
                stop_handle(handle).map_err(|error| {
                    format!("stopping retained process {}: {error}", identity.pid())
                })?;
            } else {
                stop_exact(identity).map_err(|error| {
                    format!("stopping recorded process {}: {error}", identity.pid())
                })?;
            }
        }
        let verified = self
            .root
            .canonicalize()
            .is_ok_and(|root| root.starts_with(&self.base) && root != self.base)
            && std::fs::read_to_string(self.root.join("fixture-owner"))
                .is_ok_and(|marker| marker == self.marker);
        if !verified {
            return Err("the exact fixture owner or absolute directory changed".to_owned());
        }
        self.observed.clear();
        std::fs::remove_dir_all(&self.root)
            .map_err(|error| format!("removing the stopped fixture directory: {error}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            if std::thread::panicking() {
                // Preserve the first failure while reporting the exact fixture that could not be removed.
                eprintln!(
                    "command fixture cleanup failed for {}: {error}",
                    self.root.display()
                );
            } else {
                panic!(
                    "command fixture cleanup failed for {}: {error}",
                    self.root.display()
                );
            }
        }
    }
}

#[expect(
    unsafe_code,
    reason = "tests retain and inspect exact kernel handles; no process is addressed by name"
)]
fn stop_exact(identity: ProcessIdentity) -> Result<(), String> {
    if !matches_process_start(identity.pid(), identity.started()) {
        return Ok(());
    }
    let handle = owned(
        // SAFETY: this opens only an identity recorded by this task's helper or retained child.
        unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, identity.pid()) },
        "opening an owned fixture process",
    )
    .map_err(|error| error.to_string())?;
    if process_identity(identity.pid()) != Some(identity) {
        return Err("the fixture PID changed identity before cleanup".to_owned());
    }
    stop_handle(&handle)
}

#[expect(
    unsafe_code,
    reason = "fixture cleanup uses the exact retained process handle, including after PID lookup is no longer possible"
)]
fn stop_handle(handle: &OwnedHandle) -> Result<(), String> {
    if signaled(handle) {
        return Ok(());
    }
    // SAFETY: the retained handle prevents PID reuse and the birth stamp was rechecked above.
    let terminated = unsafe { TerminateProcess(handle.as_raw_handle(), 1) };
    let failure = (terminated == 0).then(std::io::Error::last_os_error);
    // SAFETY: this exact task-owned process handle remains valid throughout the bounded wait.
    if unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) } != WAIT_OBJECT_0 {
        return Err(format!(
            "the owned fixture process did not stop: {failure:?}"
        ));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "the duplicate keeps the exact process or Job object alive for the assertion"
)]
fn duplicate(handle: HANDLE, access: u32, options: u32) -> OwnedHandle {
    let mut copied = std::ptr::null_mut();
    // SAFETY: the source is a borrowed live handle, and the new non-inheritable handle is returned once.
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &raw mut copied,
            access,
            0,
            options,
        )
    };
    assert_ne!(
        result,
        0,
        "handle duplication: {}",
        std::io::Error::last_os_error()
    );
    owned(copied, "retaining a test handle").expect("a successful duplicate is owned")
}

fn signaled(handle: &OwnedHandle) -> bool {
    wait_state(handle) == WAIT_OBJECT_0
}

#[expect(
    unsafe_code,
    reason = "the retained kernel handle reports exact process wait state without blocking"
)]
fn wait_state(handle: &OwnedHandle) -> u32 {
    // SAFETY: the handle is live and a zero wait cannot block.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) }
}

#[expect(
    unsafe_code,
    reason = "one extra suspend on the task's own not-yet-running thread induces a real resume refusal"
)]
fn add_suspend(thread: &OwnedHandle) {
    // SAFETY: primary_thread verified ownership and opened the thread with suspend rights.
    assert_eq!(unsafe { SuspendThread(thread.as_raw_handle()) }, 1);
}

#[expect(
    unsafe_code,
    reason = "Job membership is queried from the two retained kernel objects"
)]
fn in_job(job: &CommandJob, process: HANDLE) -> bool {
    let mut member = 0;
    assert_ne!(
        // SAFETY: both handles are live and member is a valid output variable.
        unsafe { IsProcessInJob(process, job.handle.as_raw_handle(), &raw mut member) },
        0
    );
    member != 0
}
