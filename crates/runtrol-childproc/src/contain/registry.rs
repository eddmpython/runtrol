//! Bounded durable guard records for Unix process groups.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, RawFd};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::contain::identity::{MAX_EXECUTABLE_BYTES, ProcessIdentity};
use crate::error::SpawnError;

const MAX_GUARDS: usize = 64;
const RECORD_MAGIC: &[u8; 8] = b"RTGRD001";
const ACTIVE_SUFFIX: &str = ".active";
const PENDING_SUFFIX: &str = ".pending";
const LOCK_FILE: &str = ".spawn.lock";
const REAP_BUDGET: Duration = Duration::from_secs(10);
const REAP_POLL: Duration = Duration::from_millis(25);
const LOCK_BUDGET: Duration = Duration::from_secs(10);
const SHUTDOWN_GATE_BUDGET: Duration = Duration::from_secs(30);
const SPAWN_GATE_POLL: Duration = Duration::from_millis(1);
static NEXT_GUARD: AtomicU64 = AtomicU64::new(0);

/// A guard identifier used only as a bounded file name inside runtrol's private directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuardId(String);

impl GuardId {
    fn mint() -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let next = NEXT_GUARD.fetch_add(1, Ordering::Relaxed);
        Self(format!("{:08x}{nanos:032x}{next:016x}", std::process::id()))
    }

    pub(super) fn parse(text: &str) -> Result<Self, SpawnError> {
        if text.len() != 56 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(failure(
                "reading a process guard",
                "the guard identifier is malformed",
            ));
        }
        Ok(Self(text.to_ascii_lowercase()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A pending durable record created before the bootstrap is spawned.
pub(super) struct PendingGuard {
    pub(super) id: GuardId,
}

/// A durable process-group registry and its cross-restart spawn lock.
#[derive(Clone, Debug)]
pub(super) struct Registry {
    inner: Arc<RegistryInner>,
}

#[derive(Debug)]
struct RegistryInner {
    directory: PathBuf,
    /// Inherited by an in-flight bootstrap. A replacement daemon cannot recover pending records until every
    /// bootstrap has either published an active record or exited.
    lock: File,
    /// Keeps the bounded create-check-create sequence atomic between threads in one daemon.
    spawn_gate: AtomicBool,
    stopping: AtomicBool,
}

pub(super) struct SpawnPermit<'a> {
    gate: &'a AtomicBool,
}

struct ShutdownPermit<'a> {
    stopping: &'a AtomicBool,
}

impl Drop for ShutdownPermit<'_> {
    fn drop(&mut self) {
        self.stopping.store(false, Ordering::Release);
    }
}

impl Drop for SpawnPermit<'_> {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}

impl Registry {
    pub(super) fn open(directory: &Path) -> Result<Self, SpawnError> {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| io_failure("creating the guard directory", error))?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| io_failure("protecting the guard directory", error))?;
        let directory = std::fs::canonicalize(directory)
            .map_err(|error| io_failure("canonicalizing the guard directory", error))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join(LOCK_FILE))
            .map_err(|error| io_failure("opening the guard lock", error))?;
        lock_exclusive(&lock)?;
        sync_directory(&directory)?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                directory,
                lock,
                spawn_gate: AtomicBool::new(false),
                stopping: AtomicBool::new(false),
            }),
        })
    }

    pub(super) fn serialize_spawn(&self) -> Result<SpawnPermit<'_>, SpawnError> {
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err(failure(
                "starting a tracked process",
                "an all-process termination is in progress",
            ));
        }
        let permit = self.acquire_spawn_gate(LOCK_BUDGET)?;
        if self.inner.stopping.load(Ordering::Acquire) {
            drop(permit);
            return Err(failure(
                "starting a tracked process",
                "an all-process termination began before the spawn boundary",
            ));
        }
        Ok(permit)
    }

    fn acquire_spawn_gate(&self, budget: Duration) -> Result<SpawnPermit<'_>, SpawnError> {
        let deadline = Instant::now() + budget;
        loop {
            if self
                .inner
                .spawn_gate
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(SpawnPermit {
                    gate: &self.inner.spawn_gate,
                });
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    "serializing a tracked process spawn",
                    "another spawn retained the registry gate for 10 seconds",
                ));
            }
            std::thread::sleep(SPAWN_GATE_POLL);
        }
    }

    pub(super) fn directory(&self) -> &Path {
        &self.inner.directory
    }

    pub(super) fn lock_fd(&self) -> RawFd {
        self.inner.lock.as_raw_fd()
    }

    pub(super) fn sync(&self) -> Result<(), SpawnError> {
        sync_directory(self.directory())
    }

    pub(super) fn create_pending(
        &self,
        _spawn_permit: &SpawnPermit<'_>,
    ) -> Result<PendingGuard, SpawnError> {
        if self.entries()?.len() >= MAX_GUARDS {
            return Err(failure(
                "creating a process guard",
                "the bounded guard directory already contains its maximum of 64 records",
            ));
        }
        for _ in 0..8 {
            let id = GuardId::mint();
            let path = self.pending_path(&id);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(RECORD_MAGIC)
                        .map_err(|error| io_failure("writing a pending process guard", error))?;
                    file.sync_all()
                        .map_err(|error| io_failure("syncing a pending process guard", error))?;
                    sync_directory(self.directory())?;
                    return Ok(PendingGuard { id });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_failure("creating a pending process guard", error)),
            }
        }
        Err(failure(
            "creating a process guard",
            "could not mint a unique bounded guard identifier",
        ))
    }

    pub(super) fn abandon_pending(&self, pending: &PendingGuard) -> Result<(), SpawnError> {
        remove_if_present(&self.pending_path(&pending.id))?;
        sync_directory(self.directory())
    }

    pub(super) fn active_guard(&self, id: GuardId, kill_on_drop: bool) -> super::ChildGuard {
        super::ChildGuard::tracked(self.clone(), id, kill_on_drop)
    }

    pub(super) fn confirm_published(&self, id: &GuardId) -> Result<(), SpawnError> {
        let active = self.active_path(id);
        let pending = self.pending_path(id);
        let pending_exists = pending
            .try_exists()
            .map_err(|error| io_failure("checking a pending process guard", error))?;
        if read_active_if_present(&active)?.is_none() || pending_exists {
            return Err(failure(
                "confirming the child bootstrap",
                "the bootstrap closed its status channel without publishing exactly one active process guard",
            ));
        }
        Ok(())
    }

    pub(super) fn publish(
        directory: &Path,
        id: &GuardId,
        identity: &ProcessIdentity,
    ) -> Result<(), SpawnError> {
        let encoded = encode(identity)?;
        let temporary = directory.join(format!(".{}.active.tmp", id.as_str()));
        let active = directory.join(format!("{}{ACTIVE_SUFFIX}", id.as_str()));
        let pending = directory.join(format!("{}{PENDING_SUFFIX}", id.as_str()));
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|error| io_failure("creating an active process guard", error))?;
            file.write_all(&encoded)
                .map_err(|error| io_failure("writing an active process guard", error))?;
            file.sync_all()
                .map_err(|error| io_failure("syncing an active process guard", error))?;
        }
        std::fs::rename(&temporary, &active)
            .map_err(|error| io_failure("publishing an active process guard", error))?;
        sync_directory(directory)?;
        remove_if_present(&pending)?;
        sync_directory(directory)
    }

    pub(super) fn recover(&self) -> Result<(), SpawnError> {
        let entries = self.entries()?;
        for entry in entries {
            match entry {
                Entry::Pending(path) | Entry::Transient(path) => remove_if_present(&path)?,
                Entry::Active { path, identity, .. } => reap(&identity, &path)?,
            }
        }
        sync_directory(self.directory())
    }

    pub(super) fn terminate_all(&self) -> Result<(), SpawnError> {
        let shutdown_permit = self.begin_shutdown()?;
        let spawn_permit = self.acquire_spawn_gate(SHUTDOWN_GATE_BUDGET)?;
        let scan = self.scan_entries()?;
        let mut errors = scan.errors;
        for entry in scan.entries {
            let result = match entry {
                Entry::Active { path, identity, .. } => {
                    signal_or_clear(&identity, &path).map(|_| ())
                }
                Entry::Pending(path) | Entry::Transient(path) => remove_if_present(&path),
            };
            if let Err(error) = result {
                errors.push(error.to_string());
            }
        }
        if let Err(error) = sync_directory(self.directory()) {
            errors.push(error.to_string());
        }
        drop(spawn_permit);
        drop(shutdown_permit);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(SpawnError::Containment {
                doing: "terminating every tracked process group",
                detail: errors.join("; "),
            })
        }
    }

    fn begin_shutdown(&self) -> Result<ShutdownPermit<'_>, SpawnError> {
        if self.inner.stopping.swap(true, Ordering::AcqRel) {
            return Err(failure(
                "terminating every tracked process group",
                "another all-process termination is already in progress",
            ));
        }
        Ok(ShutdownPermit {
            stopping: &self.inner.stopping,
        })
    }

    pub(super) fn start_terminate(&self, id: &GuardId) -> Result<bool, SpawnError> {
        let path = self.active_path(id);
        let Some(identity) = read_active_if_present(&path)? else {
            return Ok(false);
        };
        let signalled = signal_or_clear(&identity, &path)?;
        sync_directory(self.directory())?;
        Ok(signalled)
    }

    pub(super) fn finish_terminate(&self, id: &GuardId) -> Result<(), SpawnError> {
        let path = self.active_path(id);
        let Some(identity) = read_active_if_present(&path)? else {
            return Ok(());
        };
        wait_for_group_absence(identity.pid)?;
        remove_if_present(&path)?;
        sync_directory(self.directory())
    }

    pub(super) fn cleanup_failed(&self, pending: &PendingGuard) -> Result<(), SpawnError> {
        let active = self.active_path(&pending.id);
        if read_active_if_present(&active)?.is_some() {
            self.finish_terminate(&pending.id)?;
        } else {
            self.abandon_pending(pending)?;
        }
        remove_if_present(&self.pending_path(&pending.id))?;
        remove_if_present(&self.active_temporary_path(&pending.id))?;
        remove_if_present(&self.plan_path(&pending.id))?;
        remove_if_present(&self.status_path(&pending.id))?;
        sync_directory(self.directory())
    }

    pub(super) fn complete(&self, id: &GuardId) -> Result<(), SpawnError> {
        let path = self.active_path(id);
        let Some(identity) = read_active_if_present(&path)? else {
            return Ok(());
        };
        if group_exists(identity.pid)? {
            return Err(failure(
                "completing a process guard",
                "the process group still exists, so its guard must remain durable",
            ));
        }
        remove_if_present(&path)?;
        sync_directory(self.directory())
    }

    fn entries(&self) -> Result<Vec<Entry>, SpawnError> {
        let scan = self.scan_entries()?;
        if let Some(detail) = scan.errors.first() {
            return Err(SpawnError::Containment {
                doing: "reading the bounded process guard directory",
                detail: detail.clone(),
            });
        }
        Ok(scan.entries)
    }

    fn scan_entries(&self) -> Result<EntryScan, SpawnError> {
        let read = std::fs::read_dir(self.directory())
            .map_err(|error| io_failure("listing process guards", error))?;
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        for item in read {
            if entries.len() + errors.len() == MAX_GUARDS {
                errors.push("the guard directory exceeds its 64-record bound".to_owned());
                break;
            }
            let parsed = item
                .map_err(|error| io_failure("reading a process guard entry", error))
                .and_then(|entry| parse_entry(&entry));
            match parsed {
                Ok(Some(entry)) => entries.push(entry),
                Ok(None) => {}
                Err(error) => errors.push(error.to_string()),
            }
        }
        Ok(EntryScan { entries, errors })
    }

    fn pending_path(&self, id: &GuardId) -> PathBuf {
        self.directory()
            .join(format!("{}{PENDING_SUFFIX}", id.as_str()))
    }

    fn active_path(&self, id: &GuardId) -> PathBuf {
        self.directory()
            .join(format!("{}{ACTIVE_SUFFIX}", id.as_str()))
    }

    fn active_temporary_path(&self, id: &GuardId) -> PathBuf {
        self.directory()
            .join(format!(".{}.active.tmp", id.as_str()))
    }

    fn plan_path(&self, id: &GuardId) -> PathBuf {
        self.directory().join(format!(".{}.plan", id.as_str()))
    }

    fn status_path(&self, id: &GuardId) -> PathBuf {
        self.directory().join(format!(".{}.status", id.as_str()))
    }
}

enum Entry {
    Pending(PathBuf),
    Transient(PathBuf),
    Active {
        _id: GuardId,
        path: PathBuf,
        identity: ProcessIdentity,
    },
}

struct EntryScan {
    entries: Vec<Entry>,
    errors: Vec<String>,
}

fn parse_entry(item: &std::fs::DirEntry) -> Result<Option<Entry>, SpawnError> {
    let name = item.file_name();
    if name == OsStr::new(LOCK_FILE) {
        return Ok(None);
    }
    let text = name
        .to_str()
        .ok_or_else(|| failure("reading a process guard", "a guard file name is not UTF-8"))?;
    if let Some(id) = text
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".active.tmp"))
    {
        let _id = GuardId::parse(id)?;
        return Ok(Some(Entry::Transient(item.path())));
    }
    if let Some(id) = text
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".plan"))
    {
        let _id = GuardId::parse(id)?;
        return Ok(Some(Entry::Transient(item.path())));
    }
    if let Some(id) = text
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".status"))
    {
        let _id = GuardId::parse(id)?;
        return Ok(Some(Entry::Transient(item.path())));
    }
    if let Some(id) = text.strip_suffix(PENDING_SUFFIX) {
        let _id = GuardId::parse(id)?;
        return Ok(Some(Entry::Pending(item.path())));
    }
    if let Some(id) = text.strip_suffix(ACTIVE_SUFFIX) {
        let id = GuardId::parse(id)?;
        return Ok(Some(Entry::Active {
            identity: read_active(&item.path())?,
            path: item.path(),
            _id: id,
        }));
    }
    Err(failure(
        "reading a process guard",
        "the guard directory contains an unknown entry",
    ))
}

fn encode(identity: &ProcessIdentity) -> Result<Vec<u8>, SpawnError> {
    let executable = identity.executable.as_os_str().as_bytes();
    let length = u32::try_from(executable.len()).map_err(|error| SpawnError::Containment {
        doing: "encoding a process guard",
        detail: error.to_string(),
    })?;
    let mut encoded = Vec::with_capacity(24 + executable.len());
    encoded.extend_from_slice(RECORD_MAGIC);
    encoded.extend_from_slice(&identity.pid.to_le_bytes());
    encoded.extend_from_slice(&identity.start.to_le_bytes());
    encoded.extend_from_slice(&length.to_le_bytes());
    encoded.extend_from_slice(executable);
    Ok(encoded)
}

fn read_active(path: &Path) -> Result<ProcessIdentity, SpawnError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| {
            std::io::Read::by_ref(&mut file)
                .take((MAX_EXECUTABLE_BYTES + 25) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| io_failure("reading an active process guard", error))?;
    decode_active(&bytes)
}

fn read_active_if_present(path: &Path) -> Result<Option<ProcessIdentity>, SpawnError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure("reading an active process guard", error)),
    };
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take((MAX_EXECUTABLE_BYTES + 25) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| io_failure("reading an active process guard", error))?;
    decode_active(&bytes).map(Some)
}

fn decode_active(bytes: &[u8]) -> Result<ProcessIdentity, SpawnError> {
    if bytes.len() < 24 || bytes.get(..8) != Some(RECORD_MAGIC.as_slice()) {
        return Err(failure(
            "decoding a process guard",
            "the guard header is malformed",
        ));
    }
    let pid = u32::from_le_bytes(copy_array::<4>(bytes, 8)?);
    let start = u64::from_le_bytes(copy_array::<8>(bytes, 12)?);
    let length =
        usize::try_from(u32::from_le_bytes(copy_array::<4>(bytes, 20)?)).map_err(|error| {
            SpawnError::Containment {
                doing: "decoding a process guard",
                detail: error.to_string(),
            }
        })?;
    if length > MAX_EXECUTABLE_BYTES || bytes.len() != 24 + length {
        return Err(failure(
            "decoding a process guard",
            "the executable length is malformed",
        ));
    }
    let executable = PathBuf::from(std::ffi::OsString::from_vec(
        bytes.get(24..).unwrap_or_default().to_vec(),
    ));
    Ok(ProcessIdentity {
        pid,
        start,
        executable,
    })
}

fn copy_array<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], SpawnError> {
    let slice = bytes
        .get(at..at + N)
        .ok_or_else(|| failure("decoding a process guard", "the guard record is truncated"))?;
    slice
        .try_into()
        .map_err(|_| failure("decoding a process guard", "the guard record is truncated"))
}

fn reap(identity: &ProcessIdentity, path: &Path) -> Result<(), SpawnError> {
    if identity.matches_live_root()? {
        signal_group(identity.pid)?;
        wait_for_group_absence(identity.pid)?;
        remove_if_present(path)?;
        return Ok(());
    }
    if group_exists(identity.pid)? {
        return Err(failure(
            "recovering a process group",
            "the recorded root identity is gone or changed while its process group still exists",
        ));
    }
    remove_if_present(path)
}

fn signal_or_clear(identity: &ProcessIdentity, path: &Path) -> Result<bool, SpawnError> {
    if identity.matches_live_root()? {
        signal_group(identity.pid)?;
        return Ok(true);
    }
    if group_exists(identity.pid)? {
        return Err(failure(
            "terminating a process group",
            "the recorded root identity is gone or changed while its process group still exists",
        ));
    }
    remove_if_present(path)?;
    Ok(false)
}

#[expect(
    unsafe_code,
    reason = "signalling an exact Unix process group requires the kill system call"
)]
fn signal_group(group: u32) -> Result<(), SpawnError> {
    let group = i32::try_from(group).map_err(|error| SpawnError::Containment {
        doing: "signalling a process group",
        detail: error.to_string(),
    })?;
    // SAFETY: a negative PID is the documented Unix interface for signalling one process group. The group is
    // verified against its durable root identity before this call.
    let result = unsafe { libc::kill(-group, libc::SIGKILL) };
    if result == 0
        || matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        )
    {
        Ok(())
    } else {
        Err(io_failure(
            "signalling a process group",
            std::io::Error::last_os_error(),
        ))
    }
}

#[expect(
    unsafe_code,
    reason = "checking an exact Unix process group requires signal zero through the kill system call"
)]
fn group_exists(group: u32) -> Result<bool, SpawnError> {
    let group = i32::try_from(group).map_err(|error| SpawnError::Containment {
        doing: "checking a process group",
        detail: error.to_string(),
    })?;
    // SAFETY: signal zero performs existence and permission checking without delivering a signal.
    let result = unsafe { libc::kill(-group, 0) };
    if result == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(io_failure(
            "checking a process group",
            std::io::Error::last_os_error(),
        )),
    }
}

fn wait_for_group_absence(group: u32) -> Result<(), SpawnError> {
    let deadline = Instant::now() + REAP_BUDGET;
    while Instant::now() < deadline {
        if !group_exists(group)? {
            return Ok(());
        }
        std::thread::sleep(REAP_POLL);
    }
    if group_exists(group)? {
        return Err(failure(
            "waiting for a process group to end",
            "the process group remained after SIGKILL for 10 seconds",
        ));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "an inherited advisory lock requires the Unix flock system call"
)]
fn lock_exclusive(file: &File) -> Result<(), SpawnError> {
    let deadline = Instant::now() + LOCK_BUDGET;
    loop {
        // SAFETY: `file` owns a valid descriptor and `flock` neither retains nor closes it.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !error
            .raw_os_error()
            .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
        {
            return Err(io_failure("locking the guard directory", error));
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "locking the guard directory",
                "an earlier child bootstrap retained the spawn lock for 10 seconds",
            ));
        }
        std::thread::sleep(REAP_POLL);
    }
}

fn sync_directory(directory: &Path) -> Result<(), SpawnError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_failure("syncing the guard directory", error))
}

fn remove_if_present(path: &Path) -> Result<(), SpawnError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_failure("removing a process guard", error)),
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
    use std::io::Read as _;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const TEST_ROLE: &str = "RUNTROL_CONTAINMENT_TEST_ROLE";
    const TEST_ROOT: &str = "RUNTROL_CONTAINMENT_TEST_ROOT";
    const TEST_GUARD: &str = "RUNTROL_CONTAINMENT_TEST_GUARD";
    const TEST_LOCK_FD: &str = "RUNTROL_CONTAINMENT_TEST_LOCK_FD";
    const TEST_READY: &str = "RUNTROL_CONTAINMENT_TEST_READY";
    const TEST_PARENT_GONE: &str = "RUNTROL_CONTAINMENT_TEST_PARENT_GONE";
    const TEST_RELEASE: &str = "RUNTROL_CONTAINMENT_TEST_RELEASE";
    const PENDING_TEST: &str =
        "contain::registry::tests::a_pending_guard_survives_a_hard_kill_until_recovery";
    const ACTIVE_TEST: &str =
        "contain::registry::tests::an_active_guard_and_inherited_lock_survive_a_hard_kill";
    const CHILD_BUDGET: Duration = Duration::from_secs(10);
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn active_record_round_trips_the_complete_process_identity() -> Result<(), SpawnError> {
        let identity = ProcessIdentity {
            pid: 42,
            start: 7_654_321,
            executable: PathBuf::from("/opt/runtrol/provider"),
        };
        let encoded = encode(&identity)?;
        let decoded = decode_active(&encoded)?;

        assert_eq!(decoded, identity);
        Ok(())
    }

    #[test]
    fn active_record_rejects_truncation_and_unbounded_paths() -> Result<(), SpawnError> {
        let identity = ProcessIdentity {
            pid: 42,
            start: 7_654_321,
            executable: PathBuf::from("/opt/runtrol/provider"),
        };
        let encoded = encode(&identity)?;
        let mut oversized = encoded.clone();
        let Some(length_field) = oversized.get_mut(20..24) else {
            return Err(failure(
                "testing a process guard",
                "the encoded record omitted its length field",
            ));
        };
        length_field.copy_from_slice(
            &u32::try_from(MAX_EXECUTABLE_BYTES + 1)
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        let truncated = encoded.get(..encoded.len().saturating_sub(1));

        assert!(matches!(truncated, Some(bytes) if decode_active(bytes).is_err()));
        assert!(decode_active(&oversized).is_err());
        Ok(())
    }

    #[test]
    fn guard_identifier_accepts_only_one_bounded_hex_file_name() {
        assert!(GuardId::parse(&"a".repeat(56)).is_ok());
        assert!(GuardId::parse(&"a".repeat(55)).is_err());
        assert!(GuardId::parse(&format!("{}z", "a".repeat(55))).is_err());
    }

    #[test]
    fn a_pending_guard_survives_a_hard_kill_until_recovery() -> Result<(), SpawnError> {
        if test_role() == Some("pending-supervisor") {
            return hold_after_pending_publish();
        }

        let mut fixture = CrashFixture::new("pending")?;
        fixture.supervisor = Some(spawn_test_role(
            PENDING_TEST,
            "pending-supervisor",
            &fixture,
        )?);
        wait_for_file(&fixture.ready)?;
        assert_eq!(record_count(&fixture.guards, PENDING_SUFFIX)?, 1);

        fixture.kill_supervisor()?;
        let replacement = Registry::open(&fixture.guards)?;
        replacement.recover()?;
        assert_eq!(record_count(&fixture.guards, PENDING_SUFFIX)?, 0);
        assert_eq!(record_count(&fixture.guards, ACTIVE_SUFFIX)?, 0);
        fixture.finish()?;
        Ok(())
    }

    #[test]
    fn an_active_guard_and_inherited_lock_survive_a_hard_kill() -> Result<(), SpawnError> {
        match test_role() {
            Some("active-supervisor") => return hold_after_active_publish(),
            Some("active-bootstrap") => return active_bootstrap_worker(),
            _ => {}
        }

        let mut fixture = CrashFixture::new("active")?;
        fixture.supervisor = Some(spawn_test_role(ACTIVE_TEST, "active-supervisor", &fixture)?);
        wait_for_file(&fixture.ready)?;
        let root = read_process_id(&fixture.ready)?;
        fixture.process_group = Some(root);
        assert!(group_exists(root)?);
        assert_eq!(record_count(&fixture.guards, PENDING_SUFFIX)?, 0);
        assert_eq!(record_count(&fixture.guards, ACTIVE_SUFFIX)?, 1);

        fixture.kill_supervisor()?;
        wait_for_file(&fixture.parent_gone)?;
        assert_lock_is_held(&fixture.guards.join(LOCK_FILE))?;
        write_marker(&fixture.release, b"release")?;

        let replacement = Registry::open(&fixture.guards)?;
        replacement.recover()?;
        assert!(!group_exists(root)?);
        assert_eq!(record_count(&fixture.guards, PENDING_SUFFIX)?, 0);
        assert_eq!(record_count(&fixture.guards, ACTIVE_SUFFIX)?, 0);
        fixture.process_group = None;
        fixture.finish()?;
        Ok(())
    }

    fn hold_after_pending_publish() -> Result<(), SpawnError> {
        let root = required_path(TEST_ROOT)?;
        let ready = required_path(TEST_READY)?;
        let registry = Registry::open(&root.join("guards"))?;
        let spawn_permit = registry.serialize_spawn()?;
        let _pending = registry.create_pending(&spawn_permit)?;
        write_marker(&ready, b"pending")?;
        std::thread::sleep(CHILD_BUDGET);
        Err(failure(
            "testing pending recovery",
            "the hard-kill test did not stop its supervisor",
        ))
    }

    fn hold_after_active_publish() -> Result<(), SpawnError> {
        let root = required_path(TEST_ROOT)?;
        let registry = Registry::open(&root.join("guards"))?;
        let spawn_permit = registry.serialize_spawn()?;
        let pending = registry.create_pending(&spawn_permit)?;
        let executable = std::env::current_exe()
            .map_err(|error| io_failure("finding the active crash fixture", error))?;
        let lock_fd = registry.lock_fd();
        let mut command = Command::new(executable);
        command
            .args(["--exact", ACTIVE_TEST, "--nocapture", "--test-threads=1"])
            .env(TEST_ROLE, "active-bootstrap")
            .env(TEST_ROOT, &root)
            .env(TEST_GUARD, pending.id.as_str())
            .env(TEST_LOCK_FD, lock_fd.to_string())
            .env(TEST_READY, required_path(TEST_READY)?)
            .env(TEST_PARENT_GONE, required_path(TEST_PARENT_GONE)?)
            .env(TEST_RELEASE, required_path(TEST_RELEASE)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        super::super::tracked::prepare_bootstrap_descriptors(&mut command, [lock_fd]);
        let _bootstrap = command
            .spawn()
            .map_err(|error| io_failure("starting the active crash fixture", error))?;

        std::thread::sleep(CHILD_BUDGET);
        Err(failure(
            "testing active recovery",
            "the hard-kill test did not stop its supervisor",
        ))
    }

    #[expect(
        unsafe_code,
        reason = "the test subprocess reproduces the bootstrap's setpgid and inherited-lock boundary"
    )]
    fn active_bootstrap_worker() -> Result<(), SpawnError> {
        let root = required_path(TEST_ROOT)?;
        let guard = required_text(TEST_GUARD).and_then(|text| GuardId::parse(&text))?;
        let lock_fd = required_text(TEST_LOCK_FD)?
            .parse::<RawFd>()
            .map_err(|error| SpawnError::Containment {
                doing: "reading the test registry descriptor",
                detail: error.to_string(),
            })?;
        let ready = required_path(TEST_READY)?;
        let parent_gone = required_path(TEST_PARENT_GONE)?;
        let release = required_path(TEST_RELEASE)?;

        // SAFETY: this test process has no supervised descendants yet. Zero makes it the root of the exact group
        // recorded below, matching the production bootstrap order.
        if unsafe { libc::setpgid(0, 0) } != 0 {
            return Err(io_failure(
                "creating the active crash fixture group",
                std::io::Error::last_os_error(),
            ));
        }
        let executable = std::env::current_exe()
            .map_err(|error| io_failure("finding the active crash fixture", error))?;
        let identity = ProcessIdentity::current(&executable)?;
        Registry::publish(&root.join("guards"), &guard, &identity)?;
        write_marker(&ready, identity.pid.to_string().as_bytes())?;

        let mut lifeline = std::io::stdin().lock();
        let mut ignored = Vec::new();
        lifeline
            .read_to_end(&mut ignored)
            .map_err(|error| io_failure("waiting for the test supervisor to die", error))?;
        write_marker(&parent_gone, b"parent-gone")?;
        wait_for_file(&release)?;
        // SAFETY: this raw descriptor is the one inherited specifically for this test process. Closing it models
        // the production bootstrap's close-on-exec transition without adding a production failpoint.
        if unsafe { libc::close(lock_fd) } != 0 {
            return Err(io_failure(
                "closing the inherited test registry lock",
                std::io::Error::last_os_error(),
            ));
        }

        std::thread::sleep(CHILD_BUDGET);
        Err(failure(
            "testing active recovery",
            "the replacement did not terminate the published process group",
        ))
    }

    fn spawn_test_role(
        test: &str,
        role: &str,
        fixture: &CrashFixture,
    ) -> Result<Child, SpawnError> {
        let executable = std::env::current_exe()
            .map_err(|error| io_failure("finding the containment test executable", error))?;
        Command::new(executable)
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(TEST_ROLE, role)
            .env(TEST_ROOT, &fixture.root)
            .env(TEST_READY, &fixture.ready)
            .env(TEST_PARENT_GONE, &fixture.parent_gone)
            .env(TEST_RELEASE, &fixture.release)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| io_failure("starting the containment crash fixture", error))
    }

    fn test_role() -> Option<&'static str> {
        match std::env::var(TEST_ROLE).as_deref() {
            Ok("pending-supervisor") => Some("pending-supervisor"),
            Ok("active-supervisor") => Some("active-supervisor"),
            Ok("active-bootstrap") => Some("active-bootstrap"),
            _ => None,
        }
    }

    fn required_path(name: &'static str) -> Result<PathBuf, SpawnError> {
        std::env::var_os(name).map(PathBuf::from).ok_or_else(|| {
            failure(
                "reading a containment crash fixture",
                "a required test-only path is absent",
            )
        })
    }

    fn required_text(name: &'static str) -> Result<String, SpawnError> {
        std::env::var(name).map_err(|error| SpawnError::Containment {
            doing: "reading a containment crash fixture",
            detail: error.to_string(),
        })
    }

    fn wait_for_file(path: &Path) -> Result<(), SpawnError> {
        let deadline = Instant::now() + CHILD_BUDGET;
        while Instant::now() < deadline {
            if path
                .try_exists()
                .map_err(|error| io_failure("checking a containment test marker", error))?
            {
                return Ok(());
            }
            std::thread::sleep(REAP_POLL);
        }
        Err(failure(
            "waiting for a containment crash fixture",
            "the subprocess did not reach its durable boundary within 10 seconds",
        ))
    }

    fn write_marker(path: &Path, bytes: &[u8]) -> Result<(), SpawnError> {
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| io_failure("creating a containment test marker", error))?;
        marker
            .write_all(bytes)
            .and_then(|()| marker.sync_all())
            .map_err(|error| io_failure("writing a containment test marker", error))
    }

    fn read_process_id(path: &Path) -> Result<u32, SpawnError> {
        std::fs::read_to_string(path)
            .map_err(|error| io_failure("reading the active crash fixture id", error))?
            .parse::<u32>()
            .map_err(|error| SpawnError::Containment {
                doing: "parsing the active crash fixture id",
                detail: error.to_string(),
            })
    }

    fn record_count(directory: &Path, suffix: &str) -> Result<usize, SpawnError> {
        std::fs::read_dir(directory)
            .map_err(|error| io_failure("listing containment crash records", error))?
            .try_fold(0_usize, |count, item| {
                let item = item.map_err(|error| io_failure("reading a crash record", error))?;
                let matches = item
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(suffix));
                Ok(count + usize::from(matches))
            })
    }

    #[expect(
        unsafe_code,
        reason = "a nonblocking flock is the direct proof that the killed supervisor's bootstrap retained it"
    )]
    fn assert_lock_is_held(path: &Path) -> Result<(), SpawnError> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| io_failure("opening the inherited test lock", error))?;
        // SAFETY: `lock` owns the descriptor for the duration of this one nonblocking lock attempt.
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            // SAFETY: this branch acquired the lock above and releases it before returning the failed assertion.
            unsafe {
                libc::flock(lock.as_raw_fd(), libc::LOCK_UN);
            }
            return Err(failure(
                "checking the inherited registry lock",
                "the active bootstrap did not retain the lock after its supervisor died",
            ));
        }
        let error = std::io::Error::last_os_error();
        if error
            .raw_os_error()
            .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
        {
            Ok(())
        } else {
            Err(io_failure("checking the inherited registry lock", error))
        }
    }

    struct CrashFixture {
        root: PathBuf,
        guards: PathBuf,
        ready: PathBuf,
        parent_gone: PathBuf,
        release: PathBuf,
        supervisor: Option<Child>,
        process_group: Option<u32>,
        finished: bool,
    }

    impl CrashFixture {
        fn new(label: &str) -> Result<Self, SpawnError> {
            let next = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "runtrol-containment-{label}-{}-{next}",
                std::process::id()
            ));
            std::fs::create_dir(&root)
                .map_err(|error| io_failure("creating a containment crash fixture", error))?;
            let root = std::fs::canonicalize(root)
                .map_err(|error| io_failure("canonicalizing a containment crash fixture", error))?;
            Ok(Self {
                guards: root.join("guards"),
                ready: root.join("ready"),
                parent_gone: root.join("parent-gone"),
                release: root.join("release"),
                root,
                supervisor: None,
                process_group: None,
                finished: false,
            })
        }

        fn kill_supervisor(&mut self) -> Result<(), SpawnError> {
            let Some(mut supervisor) = self.supervisor.take() else {
                return Ok(());
            };
            supervisor
                .kill()
                .and_then(|()| supervisor.wait().map(|_| ()))
                .map_err(|error| io_failure("hard-killing the containment test supervisor", error))
        }

        fn finish(&mut self) -> Result<(), SpawnError> {
            std::fs::remove_dir_all(&self.root)
                .map_err(|error| io_failure("removing a containment crash fixture", error))?;
            self.finished = true;
            Ok(())
        }
    }

    impl Drop for CrashFixture {
        fn drop(&mut self) {
            if self.finished {
                return;
            }
            if let Err(error) = self.kill_supervisor() {
                eprintln!("could not stop the crash fixture supervisor: {error}");
            }
            if let Some(group) = self.process_group
                && let Err(error) = signal_group(group)
            {
                eprintln!("could not stop the crash fixture process group: {error}");
            }
            if self.root.exists()
                && let Err(error) = std::fs::remove_dir_all(&self.root)
            {
                eprintln!("could not remove the crash fixture directory: {error}");
            }
        }
    }
}
