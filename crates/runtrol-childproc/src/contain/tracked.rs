//! A command builder whose Unix spawn is durably recoverable before the provider begins.

use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::contain::Containment;
use crate::error::SpawnError;

/// A provider child whose Unix exit status is relayed by its stable process-group keeper.
pub struct TrackedChild {
    inner: tokio::process::Child,
    /// Provider standard input.
    pub stdin: Option<tokio::process::ChildStdin>,
    /// Provider standard output.
    pub stdout: Option<tokio::process::ChildStdout>,
    /// Provider standard error.
    pub stderr: Option<tokio::process::ChildStderr>,
    #[cfg(unix)]
    control: Option<std::os::unix::net::UnixStream>,
}

impl TrackedChild {
    fn direct(mut child: tokio::process::Child) -> Self {
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Self {
            inner: child,
            stdin,
            stdout,
            stderr,
            #[cfg(unix)]
            control: None,
        }
    }

    #[cfg(unix)]
    fn keeper(mut child: tokio::process::Child, control: std::os::unix::net::UnixStream) -> Self {
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Self {
            inner: child,
            stdin,
            stdout,
            stderr,
            control: Some(control),
        }
    }

    /// Process identifier of the direct child handle.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    /// Wait for the provider and return its native exit status.
    ///
    /// # Errors
    ///
    /// When the child cannot be reaped or a Unix keeper ends without its complete exit frame.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let keeper_status = self.inner.wait().await?;
        #[cfg(unix)]
        if let Some(mut control) = self.control.take() {
            use std::io::Read as _;
            use std::os::unix::process::ExitStatusExt as _;

            let mut exited = [0_u8; 5];
            control.read_exact(&mut exited).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("the process keeper ended without a provider exit frame: {error}"),
                )
            })?;
            if exited[0] != super::bootstrap::EXIT_FRAME {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the process keeper returned a malformed provider exit frame",
                ));
            }
            let raw = i32::from_le_bytes(exited[1..].try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the process keeper truncated the provider exit status",
                )
            })?);
            return Ok(std::process::ExitStatus::from_raw(raw));
        }
        Ok(keeper_status)
    }

    #[cfg(unix)]
    async fn wait_keeper(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.control.take();
        self.inner.wait().await
    }

    async fn kill(&mut self) -> std::io::Result<()> {
        self.inner.kill().await
    }
}

/// A provider-neutral child command with an optional durable containment boundary.
pub struct TrackedCommand {
    program: OsString,
    arguments: Vec<OsString>,
    current_dir: Option<PathBuf>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    kill_on_drop: bool,
}

impl TrackedCommand {
    /// Start a command description. Constructing it starts nothing.
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            arguments: Vec::new(),
            current_dir: None,
            stdin: None,
            stdout: None,
            stderr: None,
            kill_on_drop: false,
        }
    }

    /// Append one argument unchanged.
    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    /// Append arguments unchanged and in order.
    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string()),
        );
        self
    }

    /// Set the provider working directory.
    pub fn current_dir(&mut self, directory: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(directory.as_ref().to_path_buf());
        self
    }

    /// Set the provider standard input.
    pub fn stdin(&mut self, input: Stdio) -> &mut Self {
        self.stdin = Some(input);
        self
    }

    /// Set the provider standard output.
    pub fn stdout(&mut self, output: Stdio) -> &mut Self {
        self.stdout = Some(output);
        self
    }

    /// Set the provider standard error.
    pub fn stderr(&mut self, error: Stdio) -> &mut Self {
        self.stderr = Some(error);
        self
    }

    /// Ask Tokio to start killing the process when its child handle is dropped.
    pub const fn kill_on_drop(&mut self, enabled: bool) -> &mut Self {
        self.kill_on_drop = enabled;
        self
    }

    /// Spawn the command and return its process handle with the durable group guard.
    ///
    /// Production Unix containment uses the same executable's hidden stable keeper. A containment without durable
    /// tracking spawns the provider directly.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when durable publication or bootstrap execution fails, and [`SpawnError::Io`]
    /// when the operating system refuses the process spawn.
    #[cfg_attr(
        windows,
        expect(
            clippy::unused_async,
            reason = "the await lives in the Unix containment arm; the signature is one contract on both platforms"
        )
    )]
    pub async fn spawn(
        self,
        containment: &Containment,
    ) -> Result<(TrackedChild, ChildGuard), SpawnError> {
        #[cfg(unix)]
        if let Some(registry) = &containment.recovery {
            // Off the runtime thread, whole. The bootstrap path is synchronous on purpose (locks, durable
            // records, fsyncs, one keeper handshake), and on a contended CI disk those fsyncs held the
            // daemon's only async thread for 14.5 s in one piece (measured 2026-08-27 by the heartbeat
            // trace): every accept, greeting, and close request waited behind one provider spawn. A worker
            // thread pays that price alone; the registry is an `Arc` handle, so the move is a clone.
            let registry = registry.clone();
            return match tokio::task::spawn_blocking(move || self.spawn_bootstrap(&registry)).await
            {
                Ok(spawned) => spawned,
                Err(worker) => Err(SpawnError::Containment {
                    doing: "waiting for the provider spawn worker",
                    detail: worker.to_string(),
                }),
            };
        }
        self.spawn_direct(containment)
    }

    fn spawn_direct(
        self,
        containment: &Containment,
    ) -> Result<(TrackedChild, ChildGuard), SpawnError> {
        let program = self.program.clone();
        let mut command = self.into_command(&program, true, true);
        containment.prepare(command.as_std_mut());
        crate::hide_console_window(command.as_std_mut());
        let child = command.spawn().map_err(|error| SpawnError::Io {
            path: program.to_string_lossy().into_owned(),
            detail: error.to_string(),
        })?;
        Ok((TrackedChild::direct(child), ChildGuard::untracked()))
    }

    fn into_command(
        mut self,
        program: &OsStr,
        include_arguments: bool,
        include_current_dir: bool,
    ) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(program);
        crate::handoff::prepare_child_environment(command.as_std_mut());
        if include_arguments {
            command.args(&self.arguments);
        }
        command.kill_on_drop(self.kill_on_drop);
        if include_current_dir && let Some(directory) = self.current_dir.take() {
            command.current_dir(directory);
        }
        if let Some(stdin) = self.stdin.take() {
            command.stdin(stdin);
        }
        if let Some(stdout) = self.stdout.take() {
            command.stdout(stdout);
        }
        if let Some(stderr) = self.stderr.take() {
            command.stderr(stderr);
        }
        command
    }
}

#[cfg(unix)]
impl TrackedCommand {
    fn spawn_bootstrap(
        self,
        registry: &super::registry::Registry,
    ) -> Result<(TrackedChild, ChildGuard), SpawnError> {
        let spawn_permit = registry.serialize_spawn()?;
        let pending = registry.create_pending(&spawn_permit)?;
        let kill_on_drop = self.kill_on_drop;
        let result = match self.spawn_bootstrap_with_pending(registry, &pending) {
            Ok(child) => Ok((child, registry.active_guard(pending.id, kill_on_drop))),
            Err(failure) if !failure.cleanup_allowed => Err(failure.error),
            Err(failure) => match registry.cleanup_failed(&pending) {
                Ok(()) => Err(failure.error),
                Err(cleanup) => Err(SpawnError::Containment {
                    doing: "cleaning a failed child bootstrap",
                    detail: format!("{}; durable cleanup also failed: {cleanup}", failure.error),
                }),
            },
        };
        drop(spawn_permit);
        result
    }

    fn spawn_bootstrap_with_pending(
        self,
        registry: &super::registry::Registry,
        pending: &super::registry::PendingGuard,
    ) -> Result<TrackedChild, BootstrapFailure> {
        use std::io::{Seek as _, Write as _};
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let plan = super::bootstrap::LaunchPlan {
            directory: registry.directory().to_path_buf(),
            guard: pending.id.clone(),
            program: self.program.clone(),
            arguments: self.arguments.clone(),
            current_dir: self.current_dir.clone(),
        };
        let encoded = plan.encode()?;
        let plan_path = registry
            .directory()
            .join(format!(".{}.plan", pending.id.as_str()));
        let mut plan_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&plan_path)
            .map_err(|error| containment_io("creating an inherited launch plan", error))?;
        // Unlink before writing so arguments never exist through a directory entry, even if a later operation fails.
        std::fs::remove_file(&plan_path)
            .map_err(|error| containment_io("unlinking an inherited launch plan", error))?;
        registry.sync()?;
        plan_file
            .write_all(&encoded)
            .and_then(|()| plan_file.sync_all())
            .and_then(|()| plan_file.rewind())
            .map_err(|error| containment_io("writing an inherited launch plan", error))?;

        let (status_read, status_write) = status_channel(registry.directory(), &pending.id)?;
        let plan_private = duplicate_private_descriptor(
            plan_file.as_raw_fd(),
            "duplicating the child launch plan",
        )?;
        let status_private = duplicate_private_descriptor(
            status_write.as_raw_fd(),
            "duplicating the child bootstrap status channel",
        )?;
        let lock_private = duplicate_private_descriptor(
            registry.lock_fd(),
            "duplicating the child bootstrap registry lock",
        )?;
        let plan_fd = plan_private.as_raw_fd();
        let status_fd = status_private.as_raw_fd();
        let lock_fd = lock_private.as_raw_fd();
        let executable = keeper_program()?;
        let mut command = self.into_command(executable.as_os_str(), false, false);
        command.kill_on_drop(false);
        command.args([
            super::bootstrap::BOOTSTRAP_ARGUMENT.to_owned(),
            plan_fd.to_string(),
            status_fd.to_string(),
            lock_fd.to_string(),
        ]);
        crate::hide_console_window(command.as_std_mut());
        // SAFETY: the closure performs only `fcntl`, which is async-signal-safe. The three descriptors are valid in
        // the parent and inherited across fork. Clearing close-on-exec is what makes them reach the bootstrap.
        prepare_bootstrap_descriptors(command.as_std_mut(), [plan_fd, status_fd, lock_fd]);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(SpawnError::Io {
                    path: executable.to_string_lossy().into_owned(),
                    detail: error.to_string(),
                }
                .into());
            }
        };
        drop(status_write);
        drop(plan_file);
        drop((plan_private, status_private, lock_private));

        if let Err(error) = wait_for_bootstrap(&status_read) {
            drop(status_read);
            return Err(with_keeper_stop_result(&mut child, error));
        }
        if let Err(error) = registry.confirm_published(&pending.id) {
            drop(status_read);
            return Err(with_keeper_stop_result(&mut child, error));
        }
        let registry_control = match status_read.try_clone() {
            Ok(control) => control,
            Err(error) => {
                drop(status_read);
                return Err(with_keeper_stop_result(
                    &mut child,
                    containment_io("cloning the process keeper control", error),
                ));
            }
        };
        if let Err(error) = registry.register_control(pending.id.clone(), registry_control) {
            drop(status_read);
            return Err(with_keeper_stop_result(&mut child, error));
        }

        Ok(TrackedChild::keeper(child, status_read))
    }
}

/// The program name that reaches this executable's live image, not its possibly-replaced file.
///
/// On Linux the child resolves `/proc/self/exe` after fork and before exec, which names the
/// forking image itself: an update that renamed or deleted the file on disk cannot break it, while
/// a lookup through `current_exe()` returns a deleted path there and every later spawn fails
/// (the confirmed defect that blocked updates: "갱신했더니 세션이 안 열린다").
///
/// macOS has no descriptor-based exec for an unprivileged process, so the path captured at first
/// use is the honest remainder: it names the original file and an in-place update there is the
/// open design question recorded in the update initiative.
#[cfg(target_os = "linux")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the platform implementations keep one fallible signature so the spawn path cannot fork per operating system"
)]
fn keeper_program() -> Result<std::ffi::OsString, SpawnError> {
    Ok(std::ffi::OsString::from("/proc/self/exe"))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn keeper_program() -> Result<std::ffi::OsString, SpawnError> {
    static PROGRAM: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();
    if let Some(program) = PROGRAM.get() {
        return Ok(program.clone());
    }
    let found = std::env::current_exe()
        .map_err(|error| containment_io("finding the child bootstrap executable", error))?;
    Ok(PROGRAM.get_or_init(|| found.into_os_string()).clone())
}

#[cfg(unix)]
struct BootstrapFailure {
    error: SpawnError,
    cleanup_allowed: bool,
}

#[cfg(unix)]
impl From<SpawnError> for BootstrapFailure {
    fn from(error: SpawnError) -> Self {
        Self {
            error,
            cleanup_allowed: true,
        }
    }
}

#[cfg(unix)]
fn with_keeper_stop_result(
    child: &mut tokio::process::Child,
    original: SpawnError,
) -> BootstrapFailure {
    match wait_for_failed_keeper(child) {
        Ok(()) => original.into(),
        Err(stop) => BootstrapFailure {
            error: SpawnError::Containment {
                doing: "stopping a failed child bootstrap",
                detail: format!("{original}; stopping it also failed: {stop}"),
            },
            cleanup_allowed: false,
        },
    }
}

#[cfg(unix)]
fn wait_for_failed_keeper(child: &mut tokio::process::Child) -> Result<(), SpawnError> {
    if child
        .try_wait()
        .map_err(|error| containment_io("checking a failed child bootstrap", error))?
        .is_some()
    {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| containment_io("waiting for a failed child bootstrap", error))?
            .is_some()
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(SpawnError::Containment {
        doing: "waiting for a failed process keeper",
        detail: "the keeper did not close its own process group within 10 seconds".to_owned(),
    })
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "pre_exec and inherited descriptor flags require Unix APIs in the async-signal-safe fork window"
)]
pub(super) fn prepare_bootstrap_descriptors<const N: usize>(
    command: &mut std::process::Command,
    descriptors: [i32; N],
) {
    // SAFETY: the closure performs only `fcntl`, which is async-signal-safe. The descriptors are valid in the
    // parent and inherited across fork. Clearing close-on-exec is what makes them reach the bootstrap.
    unsafe {
        command.pre_exec(move || {
            for fd in descriptors {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn wait_for_bootstrap(status: &std::os::unix::net::UnixStream) -> Result<(), SpawnError> {
    // This wait is synchronous and runs on the daemon's one async thread: while it lasts, nothing else in the
    // daemon runs, including the control accept loop. The breadcrumb pair exists to prove or refute that these
    // waits are the silent stalls the Unix CI hosts show (`start` and `close --now` timing out with the daemon
    // saying nothing); the elapsed figure is printed on the way out so one slow keeper names itself.
    let bootstrap_began = std::time::Instant::now();
    contain_trace("contain: bootstrap wait began");
    let waited = wait_for_bootstrap_inner(status);
    contain_trace(&format!(
        "contain: bootstrap wait ended after {} ms",
        bootstrap_began.elapsed().as_millis()
    ));
    waited
}

/// One containment step on stderr, only when `RUNTROL_CLOSE_TRACE=1` asks for it (the CI harness does).
#[cfg(unix)]
#[expect(
    clippy::print_stderr,
    reason = "the breadcrumb exists to reach the harness's captured stderr, and only when RUNTROL_CLOSE_TRACE=1 asks for it"
)]
fn contain_trace(step: &str) {
    if std::env::var_os("RUNTROL_CLOSE_TRACE").is_some_and(|value| value == "1") {
        eprintln!("runtrol {step}");
    }
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "bounded waiting for an inherited pipe requires Unix poll"
)]
fn wait_for_bootstrap_inner(status: &std::os::unix::net::UnixStream) -> Result<(), SpawnError> {
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;

    let mut descriptor = libc::pollfd {
        fd: status.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let milliseconds = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: `descriptor` points to one initialized `pollfd`, and poll borrows it only for this bounded call.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, milliseconds) };
        if result > 0 {
            let mut status = status;
            let mut first = [0_u8; 1];
            status
                .read_exact(&mut first)
                .map_err(|error| containment_io("reading child bootstrap status", error))?;
            if first == [0] {
                return Ok(());
            }
            let mut error_text = vec![first[0]];
            std::io::Read::by_ref(&mut status)
                .take(4096)
                .read_to_end(&mut error_text)
                .map_err(|error| containment_io("reading child bootstrap failure", error))?;
            return Err(SpawnError::Containment {
                doing: "executing the supervised provider",
                detail: String::from_utf8_lossy(&error_text).into_owned(),
            });
        }
        if result == 0 {
            return Err(SpawnError::Containment {
                doing: "waiting for the child bootstrap",
                detail: "the bootstrap did not publish or fail within 10 seconds".to_owned(),
            });
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) && std::time::Instant::now() < deadline {
            continue;
        }
        return Err(containment_io("waiting for the child bootstrap", error));
    }
}

/// The durable half of one tracked child process.
pub struct ChildGuard {
    #[cfg(unix)]
    tracked: Option<TrackedGuard>,
}

#[cfg(unix)]
struct TrackedGuard {
    registry: super::registry::Registry,
    id: super::registry::GuardId,
    kill_on_drop: bool,
    finished: bool,
}

impl ChildGuard {
    const fn untracked() -> Self {
        Self {
            #[cfg(unix)]
            tracked: None,
        }
    }

    #[cfg(unix)]
    pub(super) fn tracked(
        registry: super::registry::Registry,
        id: super::registry::GuardId,
        kill_on_drop: bool,
    ) -> Self {
        Self {
            tracked: Some(TrackedGuard {
                registry,
                id,
                kill_on_drop,
                finished: false,
            }),
        }
    }

    /// Finish a naturally exited root, stop any residual descendants, and remove the durable record after no group
    /// member can execute.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when residual descendants cannot be stopped or the durable record cannot be
    /// removed.
    pub fn complete(&mut self) -> Result<(), SpawnError> {
        #[cfg(unix)]
        if let Some(tracked) = &mut self.tracked {
            tracked.registry.complete(&tracked.id)?;
            tracked.finished = true;
        }
        Ok(())
    }

    /// Close the exact keeper's private control channel, reap it, and remove the durable record after no group member
    /// can execute.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when keeper termination, reaping, or durable removal fails.
    pub async fn terminate(&mut self, child: &mut TrackedChild) -> Result<(), SpawnError> {
        #[cfg(unix)]
        if let Some(tracked) = &mut self.tracked {
            let _requested = tracked.registry.stop_keeper(&tracked.id)?;
            // Bounded: a keeper that does not answer the stop within the deadline is ended by force. Without the
            // bound this wait held a session's close forever, and every caller behind it (measured 2026-08-27 on
            // the Linux host harness: `close --now` timed out at 15 s on every trial while Windows returned at
            // once). The forced end is reported on stderr rather than as a refusal, because the close did
            // happen; what did not happen is the keeper's own cooperation, which is a diagnosis, not a failure.
            match tokio::time::timeout(KEEPER_STOP_DEADLINE, child.wait_keeper()).await {
                Ok(Ok(_exit_status)) => {}
                Ok(Err(error)) => {
                    return Err(SpawnError::Containment {
                        doing: "reaping a terminated child root",
                        detail: error.to_string(),
                    });
                }
                Err(_elapsed) => {
                    eprintln!(
                        "runtrol: a tracked child keeper did not stop within {} ms; ending it by force",
                        KEEPER_STOP_DEADLINE.as_millis()
                    );
                    child
                        .kill()
                        .await
                        .map_err(|error| SpawnError::Containment {
                            doing: "forcing a tracked child keeper to stop",
                            detail: error.to_string(),
                        })?;
                }
            }
            tracked.registry.finish_terminate(&tracked.id)?;
            tracked.finished = true;
            return Ok(());
        }
        child
            .kill()
            .await
            .map_err(|error| SpawnError::Containment {
                doing: "terminating a child process",
                detail: error.to_string(),
            })?;
        Ok(())
    }
}

/// How long a keeper is given to stop on request before it is ended by force.
#[cfg(unix)]
const KEEPER_STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(tracked) = &mut self.tracked
            && !tracked.finished
            && tracked.kill_on_drop
            && let Err(error) = tracked.registry.stop_keeper(&tracked.id)
        {
            eprintln!("runtrol: could not stop a tracked child keeper: {error}");
        }
    }
}

#[cfg(unix)]
fn status_channel(
    _directory: &Path,
    _id: &super::registry::GuardId,
) -> Result<
    (
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ),
    SpawnError,
> {
    std::os::unix::net::UnixStream::pair()
        .map_err(|error| containment_io("creating the child bootstrap control channel", error))
}

#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "F_DUPFD_CLOEXEC atomically creates a private inherited descriptor above standard input and output"
)]
fn duplicate_private_descriptor(
    descriptor: i32,
    doing: &'static str,
) -> Result<std::os::fd::OwnedFd, SpawnError> {
    use std::os::fd::FromRawFd as _;

    // SAFETY: `descriptor` is live for this call. A nonnegative result is one new owned descriptor with CLOEXEC
    // already set, and the minimum of three keeps child stdio remapping from overwriting it.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(containment_io(doing, std::io::Error::last_os_error()));
    }
    // SAFETY: successful F_DUPFD_CLOEXEC returned one fresh descriptor whose ownership transfers here.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(duplicate) })
}

#[cfg(unix)]
fn containment_io(doing: &'static str, error: impl std::fmt::Display) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: error.to_string(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn guard() -> Result<super::super::registry::GuardId, SpawnError> {
        super::super::registry::GuardId::parse(&"a".repeat(56))
    }

    #[test]
    fn explicit_keeper_ready_frame_completes_without_closing_control() -> Result<(), SpawnError> {
        let (status, mut keeper) = status_channel(Path::new("unused"), &guard()?)?;
        let writer = std::thread::spawn(move || keeper.write_all(&[0]));

        wait_for_bootstrap(&status)?;
        writer
            .join()
            .map_err(|_| containment_io("joining the ready-frame writer", "the writer panicked"))?
            .map_err(|error| containment_io("writing the ready frame", error))?;
        Ok(())
    }

    #[test]
    fn explicit_keeper_error_frame_preserves_its_diagnostic() -> Result<(), SpawnError> {
        let (status, mut keeper) = status_channel(Path::new("unused"), &guard()?)?;
        let writer = std::thread::spawn(move || keeper.write_all(b"provider exec was refused"));

        let error = wait_for_bootstrap(&status).expect_err("an error frame must refuse the spawn");
        writer
            .join()
            .map_err(|_| containment_io("joining the error-frame writer", "the writer panicked"))?
            .map_err(|error| containment_io("writing the error frame", error))?;
        assert!(error.to_string().contains("provider exec was refused"));
        Ok(())
    }
}
