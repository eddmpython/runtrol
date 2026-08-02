//! The transient Unix launch bootstrap that durably identifies itself before replacing its process image.

use std::ffi::{OsString, c_void};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;

use crate::contain::identity::ProcessIdentity;
use crate::contain::registry::{GuardId, Registry};
use crate::error::SpawnError;

/// The private argv word that selects the transient child bootstrap.
pub const BOOTSTRAP_ARGUMENT: &str = "__runtrol-child-bootstrap";
pub(super) const READY_FRAME: u8 = 0;
pub(super) const EXIT_FRAME: u8 = 1;
pub(super) const STOP_FRAME: u8 = 2;

const PLAN_MAGIC: &[u8; 8] = b"RTPLN001";
const MAX_PLAN_BYTES: usize = 128 * 1024;
const MAX_ARGUMENTS: usize = 512;
const MAX_FIELD_BYTES: usize = 32 * 1024;
const STATUS_ERROR_BYTES: usize = 4096;

/// A provider-neutral launch plan transported through an inherited, unlinked file descriptor.
pub(super) struct LaunchPlan {
    pub(super) directory: PathBuf,
    pub(super) guard: GuardId,
    pub(super) program: OsString,
    pub(super) arguments: Vec<OsString>,
    pub(super) current_dir: Option<PathBuf>,
}

impl LaunchPlan {
    pub(super) fn encode(&self) -> Result<Vec<u8>, SpawnError> {
        if self.arguments.len() > MAX_ARGUMENTS {
            return Err(failure(
                "encoding a launch plan",
                "too many provider arguments",
            ));
        }
        let mut bytes = Vec::with_capacity(MAX_PLAN_BYTES);
        put_bytes(&mut bytes, PLAN_MAGIC)?;
        put_field(&mut bytes, self.directory.as_os_str().as_bytes())?;
        put_field(&mut bytes, self.guard.as_str().as_bytes())?;
        put_field(&mut bytes, self.program.as_bytes())?;
        let count =
            u32::try_from(self.arguments.len()).map_err(|error| SpawnError::Containment {
                doing: "encoding a launch plan",
                detail: error.to_string(),
            })?;
        put_bytes(&mut bytes, &count.to_le_bytes())?;
        for argument in &self.arguments {
            put_field(&mut bytes, argument.as_bytes())?;
        }
        match &self.current_dir {
            Some(directory) => {
                put_bytes(&mut bytes, &[1])?;
                put_field(&mut bytes, directory.as_os_str().as_bytes())?;
            }
            None => put_bytes(&mut bytes, &[0])?,
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, SpawnError> {
        if bytes.len() > MAX_PLAN_BYTES || bytes.get(..8) != Some(PLAN_MAGIC.as_slice()) {
            return Err(failure(
                "decoding a launch plan",
                "the launch plan header is malformed",
            ));
        }
        let mut cursor = Cursor { bytes, at: 8 };
        let directory = PathBuf::from(OsString::from_vec(cursor.field()?.to_vec()));
        let guard_text = std::str::from_utf8(cursor.field()?).map_err(|_| {
            failure(
                "decoding a launch plan",
                "the guard identifier is not UTF-8",
            )
        })?;
        let guard = GuardId::parse(guard_text)?;
        let program = OsString::from_vec(cursor.field()?.to_vec());
        let count = usize::try_from(cursor.u32()?).map_err(|error| SpawnError::Containment {
            doing: "decoding a launch plan",
            detail: error.to_string(),
        })?;
        if count > MAX_ARGUMENTS {
            return Err(failure(
                "decoding a launch plan",
                "too many provider arguments",
            ));
        }
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(OsString::from_vec(cursor.field()?.to_vec()));
        }
        let current_dir = match cursor.byte()? {
            0 => None,
            1 => Some(PathBuf::from(OsString::from_vec(cursor.field()?.to_vec()))),
            _ => {
                return Err(failure(
                    "decoding a launch plan",
                    "the directory marker is malformed",
                ));
            }
        };
        if cursor.at != bytes.len() {
            return Err(failure(
                "decoding a launch plan",
                "the launch plan has trailing bytes",
            ));
        }
        Ok(Self {
            directory,
            guard,
            program,
            arguments,
            current_dir,
        })
    }
}

/// Run the hidden bootstrap when `words` select it.
///
/// The executable entry point must call this before interpreting public command words. `None` means this is an
/// ordinary invocation. A successful bootstrap remains as the provider's stable process-group keeper.
pub fn bootstrap_if_requested(words: &[String]) -> Option<Result<(), SpawnError>> {
    if words.first().map(String::as_str) != Some(BOOTSTRAP_ARGUMENT) {
        return None;
    }
    let status = parse_fd(words, 2);
    let result = parse_fds(words).and_then(|(plan, status, lock)| run(plan, status, lock));
    if let Err(error) = &result
        && let Ok(status) = status
    {
        write_status_error(status, error);
    }
    Some(result)
}

fn parse_fds(words: &[String]) -> Result<(RawFd, RawFd, RawFd), SpawnError> {
    if words.len() != 4 {
        return Err(failure(
            "starting the child bootstrap",
            "the private bootstrap arguments are malformed",
        ));
    }
    Ok((
        parse_fd(words, 1)?,
        parse_fd(words, 2)?,
        parse_fd(words, 3)?,
    ))
}

fn parse_fd(words: &[String], index: usize) -> Result<RawFd, SpawnError> {
    let fd = words
        .get(index)
        .ok_or_else(|| {
            failure(
                "starting the child bootstrap",
                "a private bootstrap descriptor is missing",
            )
        })?
        .parse::<RawFd>()
        .map_err(|_| {
            failure(
                "starting the child bootstrap",
                "a private bootstrap descriptor is malformed",
            )
        })?;
    if fd < 0 {
        return Err(failure(
            "starting the child bootstrap",
            "a private bootstrap descriptor is negative",
        ));
    }
    Ok(fd)
}

#[expect(
    unsafe_code,
    reason = "taking ownership of the inherited descriptor and forming a process group require Unix APIs"
)]
fn run(plan_fd: RawFd, status_fd: RawFd, lock_fd: RawFd) -> Result<(), SpawnError> {
    let mut encoded = Vec::new();
    // SAFETY: this private invocation receives ownership of the inherited plan descriptor exactly once.
    let mut plan_file = unsafe { File::from_raw_fd(plan_fd) };
    std::io::Read::by_ref(&mut plan_file)
        .take((MAX_PLAN_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|error| io_failure("reading the child launch plan", error))?;
    drop(plan_file);
    let plan = LaunchPlan::decode(&encoded)?;

    // SAFETY: zero means the calling process for both arguments. The bootstrap performs this before publishing the
    // active record, so every published keeper already anchors the group it alone may terminate.
    if unsafe { libc::setpgid(0, 0) } != 0 {
        return Err(io_failure(
            "creating the provider process group",
            std::io::Error::last_os_error(),
        ));
    }
    let keeper_executable = std::env::current_exe()
        .map_err(|error| io_failure("finding the process keeper executable", error))?;
    let identity = ProcessIdentity::current(&keeper_executable)?;
    Registry::publish(&plan.directory, &plan.guard, &identity)?;

    set_close_on_exec(status_fd)?;
    set_close_on_exec(lock_fd)?;

    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.arguments);
    if let Some(directory) = plan.current_dir {
        command.current_dir(directory);
    }
    let provider = command
        .spawn()
        .map_err(|error| io_failure("executing the supervised provider", error))?;

    // SAFETY: the private bootstrap transfers ownership of its inherited control descriptor here exactly once.
    let control = unsafe { UnixStream::from_raw_fd(status_fd) };
    if let Err(error) = keep_provider(provider, control, lock_fd) {
        eprintln!("runtrol process keeper failed: {error}");
    }
    kill_own_group()
}

fn keep_provider(
    mut provider: std::process::Child,
    mut control: UnixStream,
    lock_fd: RawFd,
) -> Result<(), SpawnError> {
    control
        .write_all(&[READY_FRAME])
        .map_err(|error| io_failure("publishing child bootstrap success", error))?;
    control
        .set_read_timeout(Some(std::time::Duration::from_millis(25)))
        .map_err(|error| io_failure("bounding the process keeper control wait", error))?;
    close_descriptor(lock_fd);

    let provider_status = loop {
        if let Some(status) = provider
            .try_wait()
            .map_err(|error| io_failure("waiting for the supervised provider", error))?
        {
            break status;
        }
        let mut command = [0_u8; 1];
        match control.read(&mut command) {
            Ok(_) => kill_own_group(),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(io_failure("reading a process keeper command", error)),
        }
    };

    let mut exited = [0_u8; 5];
    exited[0] = EXIT_FRAME;
    exited[1..].copy_from_slice(&provider_status.into_raw().to_le_bytes());
    control
        .write_all(&exited)
        .map_err(|error| io_failure("publishing the provider exit status", error))?;
    kill_own_group()
}

#[expect(
    unsafe_code,
    reason = "only a live member can atomically terminate its own Unix process group without a reused numeric target"
)]
fn kill_own_group() -> ! {
    // SAFETY: zero addresses this process's current group. The keeper is a live member at this syscall, so the group
    // generation cannot disappear or be reused before the signal is delivered to that same group.
    if unsafe { libc::kill(0, libc::SIGKILL) } != 0 {
        eprintln!(
            "runtrol process keeper could not terminate its group: {}",
            std::io::Error::last_os_error()
        );
    }
    std::process::abort()
}

#[expect(
    unsafe_code,
    reason = "the inherited global spawn lock is a raw descriptor owned by the private bootstrap"
)]
fn close_descriptor(fd: RawFd) {
    // SAFETY: the bootstrap owns this inherited duplicate and closes it exactly once after provider publication.
    let _closed = unsafe { libc::close(fd) };
}

#[expect(
    unsafe_code,
    reason = "descriptor flags have no safe standard library setter"
)]
fn set_close_on_exec(fd: RawFd) -> Result<(), SpawnError> {
    // SAFETY: the descriptor was inherited for this private bootstrap and remains open here.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io_failure(
            "reading bootstrap descriptor flags",
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: `F_SETFD` changes only the close-on-exec bit on the valid descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } != 0 {
        return Err(io_failure(
            "protecting a bootstrap descriptor",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "the bootstrap must report through its inherited raw descriptor without allocating another handle"
)]
fn write_status_error(fd: RawFd, error: &SpawnError) {
    let text = error.to_string();
    let bytes = text.as_bytes();
    let bounded = bytes
        .get(..bytes.len().min(STATUS_ERROR_BYTES))
        .unwrap_or(bytes);
    let mut written = 0;
    // SAFETY: the descriptor was supplied by the parent for this bounded status frame. `write` borrows only the
    // remaining byte range. Interrupted writes are retried, and partial writes advance within that same range.
    unsafe {
        while written < bounded.len() {
            let remaining = bounded.get_unchecked(written..);
            let result = libc::write(fd, remaining.as_ptr().cast::<c_void>(), remaining.len());
            if result > 0 {
                written = written.saturating_add(result.cast_unsigned());
                continue;
            }
            if result < 0
                && matches!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EINTR)
                )
            {
                continue;
            }
            break;
        }
        libc::close(fd);
    }
}

fn put_field(out: &mut Vec<u8>, field: &[u8]) -> Result<(), SpawnError> {
    if field.len() > MAX_FIELD_BYTES {
        return Err(failure(
            "encoding a launch plan",
            "one launch field exceeds 32 KiB",
        ));
    }
    let length = u32::try_from(field.len()).map_err(|error| SpawnError::Containment {
        doing: "encoding a launch plan",
        detail: error.to_string(),
    })?;
    put_bytes(out, &length.to_le_bytes())?;
    put_bytes(out, field)?;
    Ok(())
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SpawnError> {
    let Some(length) = out.len().checked_add(bytes.len()) else {
        return Err(failure(
            "encoding a launch plan",
            "the launch plan length overflowed",
        ));
    };
    if length > MAX_PLAN_BYTES {
        return Err(failure(
            "encoding a launch plan",
            "the launch plan exceeds 128 KiB",
        ));
    }
    out.extend_from_slice(bytes);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], SpawnError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or_else(|| failure("decoding a launch plan", "a field length overflowed"))?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| failure("decoding a launch plan", "the launch plan is truncated"))?;
        self.at = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, SpawnError> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| failure("decoding a launch plan", "the launch plan is truncated"))
    }

    fn u32(&mut self) -> Result<u32, SpawnError> {
        let value: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| failure("decoding a launch plan", "the launch plan is truncated"))?;
        Ok(u32::from_le_bytes(value))
    }

    fn field(&mut self) -> Result<&'a [u8], SpawnError> {
        let length = usize::try_from(self.u32()?).map_err(|error| SpawnError::Containment {
            doing: "decoding a launch plan",
            detail: error.to_string(),
        })?;
        if length > MAX_FIELD_BYTES {
            return Err(failure(
                "decoding a launch plan",
                "one launch field exceeds 32 KiB",
            ));
        }
        self.take(length)
    }
}

fn io_failure(doing: &'static str, error: impl std::fmt::Display) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: error.to_string(),
    }
}

fn failure(doing: &'static str, detail: &'static str) -> SpawnError {
    SpawnError::Containment {
        doing,
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt as _;

    use super::*;

    fn plan() -> Result<LaunchPlan, SpawnError> {
        Ok(LaunchPlan {
            directory: PathBuf::from("/tmp/runtrol-guards"),
            guard: GuardId::parse(&"a".repeat(56))?,
            program: OsString::from("/bin/provider"),
            arguments: vec![OsString::from_vec(vec![b'a', 0xff, b'b'])],
            current_dir: Some(PathBuf::from("/tmp/project")),
        })
    }

    #[test]
    fn launch_plan_round_trips_non_utf8_arguments_without_a_shell() -> Result<(), SpawnError> {
        let original = plan()?;
        let encoded = original.encode()?;
        let decoded = LaunchPlan::decode(&encoded)?;

        assert_eq!(decoded.directory, original.directory);
        assert_eq!(decoded.guard, original.guard);
        assert_eq!(decoded.program, original.program);
        assert_eq!(decoded.arguments, original.arguments);
        assert_eq!(decoded.current_dir, original.current_dir);
        Ok(())
    }

    #[test]
    fn launch_plan_rejects_trailing_and_truncated_bytes() -> Result<(), SpawnError> {
        let encoded = plan()?.encode()?;
        let mut trailing = encoded.clone();
        trailing.push(0);
        let truncated = encoded.get(..encoded.len().saturating_sub(1));

        assert!(LaunchPlan::decode(&trailing).is_err());
        assert!(matches!(truncated, Some(bytes) if LaunchPlan::decode(bytes).is_err()));
        Ok(())
    }

    #[test]
    fn launch_plan_enforces_its_argument_bound_before_allocating_a_record() -> Result<(), SpawnError>
    {
        let mut too_many = plan()?;
        too_many.arguments = (0..=MAX_ARGUMENTS).map(|_| OsString::from("x")).collect();

        assert!(too_many.encode().is_err());
        Ok(())
    }

    #[test]
    fn launch_plan_rejects_aggregate_bytes_at_the_fixed_capacity() -> Result<(), SpawnError> {
        let mut too_wide = plan()?;
        too_wide.arguments = (0..5)
            .map(|_| OsString::from_vec(vec![b'x'; MAX_FIELD_BYTES]))
            .collect();

        assert!(too_wide.encode().is_err());
        Ok(())
    }
}
