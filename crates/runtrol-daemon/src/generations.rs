//! Daemon generations: one build, one identity, side by side with the build it replaces.
//!
//! # The model
//!
//! A generation is one running build of the daemon, identified by the SHA-256 of its executable. It binds
//! endpoints that carry that digest, so no two builds ever contend for a name, and it publishes itself into the
//! one locator file of the home. A newer build does not stop the older one: it starts beside it, asks it to
//! drain, and takes over the durable store the moment the older one releases it. The older generation keeps
//! serving the turns already running and exits by itself once none is left.
//!
//! What this removes: a retire request that could be refused, an idle judgement, a mid-turn judgement, a binary
//! written over the one a process is running, and the gap between one daemon leaving and the next arriving.
//!
//! # Who writes the locator
//!
//! Only daemons, only their own entry, only under the home's advisory lock. Readers take no lock: every write is
//! an atomic rename of a complete file. A dead entry (the process behind it no longer answers on its control
//! endpoint) is dropped by the next generation that publishes, so a crash leaves nothing a reader has to doubt
//! for long.

use core::time::Duration;
use std::path::{Path, PathBuf};

use runtrol_core::{Layout, RuntrolHome};
use runtrol_ipc::frame::WIRE_VERSION;
use runtrol_ipc::transport::{Connection, TransportError};
use runtrol_ipc::wire::{Request, Response};
use runtrol_provider::AbsPath;
use runtrol_runtime_protocol::{
    RUNTIME_LOCATOR_SCHEMA, RuntimeEndpointKind, RuntimeGeneration, RuntimeLocatorRecord,
};
use serde::{Deserialize, Serialize};

use crate::compose::{ComposeError, Composed};

const MAX_RECORD_BYTES: u64 = 16 * 1024;

/// Schema of the durable instance record. Separate from the locator's: the instance outlives locator shapes.
const RUNTIME_INSTANCE_SCHEMA: u32 = 1;

/// How long a starting generation waits for its predecessor to hand over the store.
///
/// A generation that knows how to drain releases the store within milliseconds of being asked. The only slow
/// case is a daemon built before generations existed: it can only be asked to retire, refuses while a turn
/// runs, and is asked again until the turn ends.
const STORE_HANDOVER_DEADLINE: Duration = Duration::from_mins(10);

/// How often a starting generation asks again while its predecessor still holds the store.
const STORE_HANDOVER_RETRY: Duration = Duration::from_secs(2);

/// Runtime identity, locator publication, or cleanup failed closed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeBootstrapError {
    #[error("cannot read Runtime bootstrap state at {path}: {detail}")]
    Read { path: String, detail: String },
    #[error("Runtime bootstrap state at {path} is unsafe: {why}")]
    Unsafe { path: String, why: &'static str },
    #[error("Runtime bootstrap state at {path} is malformed: {detail}")]
    Malformed { path: String, detail: String },
    #[error("cannot write Runtime bootstrap state at {path}: {detail}")]
    Write { path: String, detail: String },
    #[error("operating-system randomness is unavailable for the Runtime instance identity")]
    Random,
}

/// The build this process is: its full digest, and the sixteen-digit tag its endpoints are named by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationIdentity {
    digest: String,
    tag: String,
}

impl GenerationIdentity {
    /// This process's own generation.
    ///
    /// # Errors
    ///
    /// The executable could not be read back, which leaves nothing to name an endpoint by.
    pub fn of_this_executable() -> Result<Self, ComposeError> {
        let Some(digest) = crate::build_identity::build_digest() else {
            return Err(ComposeError::Identity {
                path: std::env::current_exe().map_or_else(
                    |_| "<unknown>".to_owned(),
                    |path| path.display().to_string(),
                ),
                detail: "the image could not be read back".to_owned(),
            });
        };
        Ok(Self::of_digest(digest))
    }

    /// The generation one executable file would serve under.
    ///
    /// # Errors
    ///
    /// The file could not be read.
    pub fn of_executable(executable: &Path) -> Result<Self, ComposeError> {
        let digest = crate::build_identity::digest_of(executable).map_err(|error| {
            ComposeError::Identity {
                path: executable.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        Ok(Self::of_digest(&digest))
    }

    fn of_digest(digest: &str) -> Self {
        Self {
            digest: digest.to_owned(),
            tag: digest
                .chars()
                .take(runtrol_core::GENERATION_TAG_LENGTH)
                .collect(),
        }
    }

    /// Full SHA-256, lowercase hex.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The sixteen leading hex digits endpoints are named by.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }
}

/// Where one executable's generation listens for private control, for the command surface that runs it.
///
/// Both ends derive this from the same executable bytes and the same home, so a command connects to exactly
/// the build it is, and starts that build when nothing listens there.
///
/// # Errors
///
/// [`ComposeError::Home`] when runtrol's directory cannot be established, [`ComposeError::Identity`] when the
/// executable cannot be read.
pub fn generation_endpoint(home: Option<&str>, executable: &Path) -> Result<String, ComposeError> {
    let home = match home {
        Some(chosen) => RuntrolHome::open_at(chosen)?,
        None => RuntrolHome::open()?,
    };
    let identity = GenerationIdentity::of_executable(executable)?;
    Ok(home
        .paths()
        .generation_endpoint(identity.tag())?
        .address()
        .to_owned())
}

/// Where this home's daemon records a panic, for the daemon personality to install its hook before assembly.
///
/// # Errors
///
/// [`ComposeError::Home`] when runtrol's directory cannot be established.
pub fn crash_log_path(home: Option<&str>) -> Result<PathBuf, ComposeError> {
    let home = match home {
        Some(chosen) => RuntrolHome::open_at(chosen)?,
        None => RuntrolHome::open()?,
    };
    Ok(home.paths().daemon_crash_log().as_std_path().to_owned())
}

/// Assemble a daemon after asking every earlier generation of this home to hand over the store.
///
/// Composition opens the exclusive store, so the predecessors are asked first and the open is retried while
/// one of them still holds the file. Nothing is killed: a predecessor releases the store the moment it is
/// asked, or, for a build older than generations, when its running turns end.
///
/// # Errors
///
/// Every [`ComposeError`], with [`runtrol_store::StoreError::AlreadyOpen`] reported only after the handover
/// deadline passed with the store still held.
pub async fn assemble_superseding(
    builtin: runtrol_drivers::Builtin,
    identity: &GenerationIdentity,
) -> Result<Composed, ComposeError> {
    let home = RuntrolHome::open()?;
    let deadline = tokio::time::Instant::now() + STORE_HANDOVER_DEADLINE;
    loop {
        drain_predecessors(home.paths(), identity.digest()).await;
        match Composed::assemble(None, builtin) {
            // Either exclusive file still held by a predecessor means the handover is in flight, not failed.
            Err(
                ComposeError::Store(runtrol_store::StoreError::AlreadyOpen { .. })
                | ComposeError::Ledger(runtrol_ledger::LedgerError::AlreadyOpen),
            ) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(STORE_HANDOVER_RETRY).await;
            }
            outcome => return outcome,
        }
    }
}

/// Ask every generation of this home other than `own_digest`, and any daemon built before generations, to
/// hand over the store. Best effort: a predecessor that has already gone is exactly the outcome wanted.
async fn drain_predecessors(paths: &Layout, own_digest: &str) {
    match read_locator(paths.runtime_locator().as_std_path()) {
        Ok(Located::Current(record)) => {
            for generation in &record.generations {
                if generation.digest == own_digest || generation.draining {
                    continue;
                }
                // ok: an unreachable or refusing predecessor changes nothing here. Either it is already gone,
                // or the store open fails and is retried until the deadline names the holder.
                drop(ask_once(&generation.control_endpoint, &Request::Drain).await);
            }
        }
        // A daemon from before generations published a locator of the earlier shape, listens on the bare home
        // endpoint, and knows only `retire`, which it refuses mid-turn. Asked again on every retry, it exits
        // at the first idle moment. This arm is the whole compatibility surface with that era and goes with it.
        // ok: the same reasoning as above; the retry loop is the handling.
        Ok(Located::Legacy) => drop(ask_legacy_retire(paths.endpoint().address()).await),
        // Nothing published means nothing to ask: no daemon of any generation serves this home.
        Ok(Located::Absent) | Err(_) => {}
    }
}

async fn ask_once(control_endpoint: &str, request: &Request) -> Result<(), TransportError> {
    let mut connection = runtrol_ipc::transport::connect(control_endpoint).await?;
    greet(&mut connection).await?;
    send(&mut connection, request).await?;
    drop(connection.recv().await?);
    Ok(())
}

async fn ask_legacy_retire(bare_endpoint: &str) -> Result<(), TransportError> {
    let mut connection = runtrol_ipc::transport::connect(bare_endpoint).await?;
    greet(&mut connection).await?;
    connection.send(br#"{"ask":"retire"}"#).await?;
    drop(connection.recv().await?);
    Ok(())
}

async fn greet(connection: &mut Connection) -> Result<(), TransportError> {
    send(connection, &Request::Hello { wire: WIRE_VERSION }).await?;
    drop(connection.recv().await?);
    Ok(())
}

async fn send(connection: &mut Connection, request: &Request) -> Result<(), TransportError> {
    let encoded = serde_json::to_vec(request).map_err(|error| TransportError::Io {
        doing: "encoding a generation request",
        detail: error.to_string(),
    })?;
    connection.send(&encoded).await
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstanceRecord {
    schema: u32,
    instance_id: String,
}

/// Load the durable installed-Runtime identity or mint it exactly once for a new home.
pub(crate) fn load_or_create_instance(path: &AbsPath) -> Result<String, RuntimeBootstrapError> {
    if path.as_std_path().exists() {
        let record: InstanceRecord = read_bounded(path.as_std_path())?;
        if record.schema != RUNTIME_INSTANCE_SCHEMA || !valid_instance(&record.instance_id) {
            return Err(RuntimeBootstrapError::Malformed {
                path: path.as_str().to_owned(),
                detail: "unsupported schema or invalid instance identity".to_owned(),
            });
        }
        return Ok(record.instance_id);
    }

    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| RuntimeBootstrapError::Random)?;
    let mut instance_id = String::with_capacity(4 + random.len() * 2);
    instance_id.push_str("rtm_");
    for byte in random {
        use core::fmt::Write as _;
        write!(&mut instance_id, "{byte:02x}").map_err(|error| RuntimeBootstrapError::Write {
            path: path.as_str().to_owned(),
            detail: error.to_string(),
        })?;
    }
    write_new(
        path.as_std_path(),
        &InstanceRecord {
            schema: RUNTIME_INSTANCE_SCHEMA,
            instance_id: instance_id.clone(),
        },
    )?;
    Ok(instance_id)
}

/// This generation's entry in the home locator, for as long as this value lives.
///
/// Publishing inserts the entry beside every other live generation. Dropping removes exactly this entry and
/// leaves the others. `update` keeps the entry's live-turn count and draining flag current so `runtrol status`
/// and clients read the truth without asking the process.
pub(crate) struct PublishedGeneration {
    locator: PathBuf,
    lock: PathBuf,
    digest: String,
    process_id: u32,
    published: RuntimeGeneration,
}

impl PublishedGeneration {
    /// Publish after both endpoints are already bound.
    ///
    /// Entries of generations that no longer answer on their control endpoint are dropped here, which is the
    /// only place a stale entry is ever removed by somebody other than its owner.
    pub(crate) async fn publish(
        paths: &Layout,
        instance_id: &str,
        identity: &GenerationIdentity,
        endpoint: &str,
        control_endpoint: &str,
    ) -> Result<Self, RuntimeBootstrapError> {
        let locator = paths.runtime_locator().as_std_path().to_owned();
        let lock = paths.runtime_locator_lock().as_std_path().to_owned();
        let process_id = std::process::id();
        let published = RuntimeGeneration {
            digest: identity.digest().to_owned(),
            endpoint_kind: endpoint_kind(),
            endpoint: endpoint.to_owned(),
            control_endpoint: control_endpoint.to_owned(),
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            process_id,
            started_at_ms: now_ms(),
            live_sessions: 0,
            draining: false,
        };
        let _held = LocatorLock::take(&lock)?;
        let mut record = current_record(&locator)?.unwrap_or_else(|| RuntimeLocatorRecord {
            schema: RUNTIME_LOCATOR_SCHEMA,
            instance_id: instance_id.to_owned(),
            generations: Vec::new(),
        });
        record.instance_id = instance_id.to_owned();
        let mut kept = Vec::with_capacity(record.generations.len() + 1);
        for generation in record.generations {
            // An entry with this process id and this digest is a stale record of a reused pid.
            let stale_self =
                generation.process_id == process_id && generation.digest == published.digest;
            // An entry on the control endpoint this daemon just bound is a dead predecessor: the endpoint is
            // exclusive, so this daemon could only bind it because whoever held it before is gone. Its own
            // answer on that shared address would otherwise keep the dead entry alive forever (measured
            // 2026-08-25: a same-digest restart left two entries, one a dead pid, in `runtrol status`).
            let my_endpoint = generation.control_endpoint == published.control_endpoint;
            if stale_self || my_endpoint || !answers(&generation.control_endpoint).await {
                continue;
            }
            kept.push(generation);
        }
        kept.push(published.clone());
        record.generations = kept;
        write_locator(&locator, &record)?;
        Ok(Self {
            locator,
            lock,
            digest: identity.digest().to_owned(),
            process_id,
            published,
        })
    }

    /// Record how many turns are still running here and whether a successor has taken over.
    ///
    /// Writes only on a change: the owner loop calls this on every index publish.
    pub(crate) fn update(&mut self, live_sessions: u32, draining: bool) {
        if self.published.live_sessions == live_sessions && self.published.draining == draining {
            return;
        }
        self.published.live_sessions = live_sessions;
        self.published.draining = draining;
        // ok: a locator that cannot be updated is diagnosed by `runtrol status` reading a stale count, and
        // nothing about serving depends on the file; the next publish or drop retries the same write.
        drop(self.rewrite(|record| {
            if let Some(mine) = record.generations.iter_mut().find(|generation| {
                generation.process_id == self.process_id && generation.digest == self.digest
            }) {
                mine.live_sessions = live_sessions;
                mine.draining = draining;
            }
        }));
    }

    fn rewrite(
        &self,
        change: impl FnOnce(&mut RuntimeLocatorRecord),
    ) -> Result<(), RuntimeBootstrapError> {
        let _held = LocatorLock::take(&self.lock)?;
        let Some(mut record) = current_record(&self.locator)? else {
            return Ok(());
        };
        change(&mut record);
        write_locator(&self.locator, &record)
    }
}

impl Drop for PublishedGeneration {
    fn drop(&mut self) {
        // ok: an entry that cannot be removed at exit is dropped by the next generation's publish, which
        // probes every listed control endpoint; nothing can act on this process after it is gone anyway.
        drop(self.rewrite(|record| {
            record.generations.retain(|generation| {
                !(generation.process_id == self.process_id && generation.digest == self.digest)
            });
        }));
    }
}

/// Whether a daemon still answers on a control endpoint. A refusal to connect for any reason other than
/// nothing listening keeps the entry: a busy pipe is still a live daemon.
async fn answers(control_endpoint: &str) -> bool {
    match runtrol_ipc::transport::connect(control_endpoint).await {
        Ok(connection) => {
            drop(connection);
            true
        }
        Err(error) => !error.means_no_daemon(),
    }
}

/// One generation as `runtrol status` reports it: the locator entry plus whether it answers right now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationStatus {
    /// The locator entry.
    #[serde(flatten)]
    pub generation: RuntimeGeneration,
    /// Whether its control endpoint accepted a connection just now.
    pub answering: bool,
}

/// Every generation listed for this home, probed for liveness. Starts nothing.
///
/// # Errors
///
/// [`ComposeError::Home`] when runtrol's directory cannot be established.
pub async fn status(home: Option<&str>) -> Result<Vec<GenerationStatus>, ComposeError> {
    let home = match home {
        Some(chosen) => RuntrolHome::open_at(chosen)?,
        None => RuntrolHome::open()?,
    };
    let record = current_record(home.paths().runtime_locator().as_std_path()).map_err(|error| {
        ComposeError::Identity {
            path: home.paths().runtime_locator().as_str().to_owned(),
            detail: error.to_string(),
        }
    })?;
    let mut statuses = Vec::new();
    for generation in record.map(|record| record.generations).unwrap_or_default() {
        let answering = answers(&generation.control_endpoint).await;
        statuses.push(GenerationStatus {
            generation,
            answering,
        });
    }
    Ok(statuses)
}

/// What the locator file holds.
pub(crate) enum Located {
    /// No file: no daemon of any generation serves this home.
    Absent,
    /// A locator of another shape: a daemon from before generations, or a later build this one does not read.
    Legacy,
    /// The generations this build can act on.
    Current(RuntimeLocatorRecord),
}

/// Read the locator without taking the lock; every write is an atomic rename of a complete file.
///
/// # Errors
///
/// The file exists and cannot be trusted or read.
pub(crate) fn read_locator(path: &Path) -> Result<Located, RuntimeBootstrapError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Located::Absent),
        Err(error) => {
            return Err(RuntimeBootstrapError::Read {
                path: path.display().to_string(),
                detail: error.to_string(),
            });
        }
    }
    match read_bounded::<RuntimeLocatorRecord>(path) {
        Ok(record) if record.schema == RUNTIME_LOCATOR_SCHEMA => Ok(Located::Current(record)),
        // Neither shape can be merged into; the next publish replaces it whole.
        Ok(_) | Err(RuntimeBootstrapError::Malformed { .. }) => Ok(Located::Legacy),
        Err(error) => Err(error),
    }
}

/// The generations listed right now, or none. What `status`, `publish` and the tests read.
fn current_record(path: &Path) -> Result<Option<RuntimeLocatorRecord>, RuntimeBootstrapError> {
    Ok(match read_locator(path)? {
        Located::Current(record) => Some(record),
        Located::Absent | Located::Legacy => None,
    })
}

fn write_locator(path: &Path, record: &RuntimeLocatorRecord) -> Result<(), RuntimeBootstrapError> {
    if record.generations.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RuntimeBootstrapError::Write {
                path: path.display().to_string(),
                detail: error.to_string(),
            }),
        };
    }
    ensure_replaceable(path)?;
    write_new(path, record)
}

/// The advisory lock around one locator read-modify-write. Released on drop.
struct LocatorLock(std::fs::File);

impl LocatorLock {
    fn take(path: &Path) -> Result<Self, RuntimeBootstrapError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|error| RuntimeBootstrapError::Write {
                path: path.display().to_string(),
                detail: error.to_string(),
            })?;
        file.lock().map_err(|error| RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: format!("cannot take the locator lock: {error}"),
        })?;
        Ok(Self(file))
    }
}

impl Drop for LocatorLock {
    fn drop(&mut self) {
        // ok: an unlock that fails is released by the operating system when the handle closes on the next line.
        drop(self.0.unlock());
    }
}

fn now_ms() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        // A clock before the epoch orders this generation first; the process id still tells entries apart.
        Err(_) => 0,
    }
}

fn read_bounded<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RuntimeBootstrapError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| RuntimeBootstrapError::Read {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the record is not a regular file",
        });
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the record exceeds its byte limit",
        });
    }
    let bytes = std::fs::read(path).map_err(|error| RuntimeBootstrapError::Read {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| RuntimeBootstrapError::Malformed {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

fn write_new<T: Serialize>(path: &Path, value: &T) -> Result<(), RuntimeBootstrapError> {
    let encoded = serde_json::to_vec(value).map_err(|error| RuntimeBootstrapError::Write {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    if u64::try_from(encoded.len()).map_or(true, |length| length > MAX_RECORD_BYTES) {
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: "the encoded record exceeds its byte limit".to_owned(),
        });
    }
    let pending = random_sibling(path)?;
    runtrol_ipc::transport::create_owner_only_file(&pending, &encoded).map_err(|error| {
        RuntimeBootstrapError::Write {
            path: pending.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let outcome = std::fs::rename(&pending, path);
    if let Err(error) = outcome {
        drop(std::fs::remove_file(&pending));
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: error.to_string(),
        });
    }
    Ok(())
}

fn ensure_replaceable(path: &Path) -> Result<(), RuntimeBootstrapError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the stale locator is not a regular file",
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeBootstrapError::Read {
            path: path.display().to_string(),
            detail: error.to_string(),
        }),
    }
}

fn random_sibling(path: &Path) -> Result<PathBuf, RuntimeBootstrapError> {
    let Some(name) = path.file_name() else {
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: "the record path has no file name".to_owned(),
        });
    };
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| RuntimeBootstrapError::Random)?;
    let mut suffix = String::with_capacity(random.len() * 2);
    for byte in random {
        use core::fmt::Write as _;
        write!(&mut suffix, "{byte:02x}").map_err(|error| RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
    }
    let mut pending = name.to_os_string();
    pending.push(format!(".{suffix}.new"));
    Ok(path.with_file_name(pending))
}

fn valid_instance(instance_id: &str) -> bool {
    instance_id.len() == 36
        && instance_id.starts_with("rtm_")
        && instance_id
            .bytes()
            .skip(4)
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(windows)]
const fn endpoint_kind() -> RuntimeEndpointKind {
    RuntimeEndpointKind::NamedPipe
}

#[cfg(unix)]
const fn endpoint_kind() -> RuntimeEndpointKind {
    RuntimeEndpointKind::UnixSocket
}

/// The one answer a drained predecessor gives, kept beside the request so the two cannot drift.
pub(crate) fn drained() -> Response {
    Response::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn make(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "runtrol-generations-scratch-{name}-{}",
                std::process::id()
            ));
            drop(std::fs::remove_dir_all(&path));
            std::fs::create_dir_all(&path).expect("create scratch");
            Self(path)
        }

        fn layout(&self) -> Layout {
            RuntrolHome::open_at(self.0.to_str().expect("UTF-8 scratch"))
                .expect("open scratch home")
                .paths()
                .clone()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    const INSTANCE: &str = "rtm_0123456789abcdef0123456789abcdef";

    fn identity(byte: u8) -> GenerationIdentity {
        GenerationIdentity::of_digest(&format!("{byte:02x}").repeat(32))
    }

    #[test]
    fn installed_instance_survives_restart_and_full_removal_mints_another() {
        let scratch = Scratch::make("instance");
        let path = scratch.layout().runtime_instance().clone();
        let first = load_or_create_instance(&path).expect("mint instance");
        let second = load_or_create_instance(&path).expect("restore instance");
        assert_eq!(first, second);
        std::fs::remove_file(path.as_std_path()).expect("simulate uninstall");
        let third = load_or_create_instance(&path).expect("mint after reinstall");
        assert_ne!(first, third);
    }

    /// A control endpoint something really listens on, so the liveness probe keeps its entry.
    async fn listening(
        scratch: &Scratch,
        name: &str,
    ) -> (runtrol_ipc::transport::Listener, String) {
        let address = if cfg!(windows) {
            format!(
                r"\\.\pipe\runtrol-generations-{name}-{}",
                std::process::id()
            )
        } else {
            scratch
                .0
                .join(format!("{name}.sock"))
                .to_string_lossy()
                .into_owned()
        };
        let listener = runtrol_ipc::transport::Listener::bind(&address)
            .await
            .expect("bind a test control endpoint");
        (listener, address)
    }

    #[tokio::test]
    async fn two_generations_share_one_locator_and_each_removes_only_itself() {
        let scratch = Scratch::make("side-by-side");
        let layout = scratch.layout();
        let (_listen_a, control_a) = listening(&scratch, "control-a").await;
        let (_listen_b, control_b) = listening(&scratch, "control-b").await;
        let older = PublishedGeneration::publish(
            &layout,
            INSTANCE,
            &identity(0xaa),
            "public-a",
            &control_a,
        )
        .await
        .expect("publish the older generation");
        let mut newer = PublishedGeneration::publish(
            &layout,
            INSTANCE,
            &identity(0xbb),
            "public-b",
            &control_b,
        )
        .await
        .expect("publish the newer generation");
        let record = current_record(layout.runtime_locator().as_std_path())
            .expect("read")
            .expect("present");
        assert_eq!(
            record.generations.len(),
            2,
            "both live generations are listed"
        );
        assert_eq!(
            record.current().map(|g| g.digest.as_str()),
            Some(identity(0xbb).digest())
        );

        newer.update(3, true);
        let record = current_record(layout.runtime_locator().as_std_path())
            .expect("read")
            .expect("present");
        let entry = record
            .with_digest(identity(0xbb).digest())
            .expect("newer listed");
        assert_eq!((entry.live_sessions, entry.draining), (3, true));
        assert_eq!(
            record.current().map(|g| g.digest.as_str()),
            Some(identity(0xaa).digest())
        );

        drop(older);
        let record = current_record(layout.runtime_locator().as_std_path())
            .expect("read")
            .expect("present");
        assert_eq!(record.generations.len(), 1);
        assert_eq!(
            record.generations.first().map(|g| g.digest.as_str()),
            Some(identity(0xbb).digest())
        );
        drop(newer);
        assert!(
            !layout.runtime_locator().as_std_path().exists(),
            "the last generation removes the file"
        );
    }

    #[tokio::test]
    async fn a_dead_entry_is_dropped_by_the_next_publish() {
        let scratch = Scratch::make("dead-entry");
        let layout = scratch.layout();
        let dead = RuntimeLocatorRecord {
            schema: RUNTIME_LOCATOR_SCHEMA,
            instance_id: INSTANCE.to_owned(),
            generations: vec![RuntimeGeneration {
                digest: identity(0x11).digest().to_owned(),
                endpoint_kind: endpoint_kind(),
                endpoint: "public-dead".to_owned(),
                control_endpoint: if cfg!(windows) {
                    r"\\.\pipe\runtrol-generations-test-nobody-listens".to_owned()
                } else {
                    scratch.0.join("nobody.sock").to_string_lossy().into_owned()
                },
                runtime_version: "0.0.0".to_owned(),
                process_id: 1,
                started_at_ms: 1,
                live_sessions: 0,
                draining: false,
            }],
        };
        write_locator(layout.runtime_locator().as_std_path(), &dead).expect("seed a dead entry");
        let live = PublishedGeneration::publish(
            &layout,
            INSTANCE,
            &identity(0x22),
            "public-live",
            "control-live",
        )
        .await
        .expect("publish");
        let record = current_record(layout.runtime_locator().as_std_path())
            .expect("read")
            .expect("present");
        assert_eq!(
            record.generations.len(),
            1,
            "the entry nobody answers for is gone"
        );
        assert_eq!(
            record.generations.first().map(|g| g.digest.as_str()),
            Some(identity(0x22).digest())
        );
        drop(live);
    }

    #[tokio::test]
    async fn a_same_digest_restart_replaces_the_dead_entry_on_its_own_endpoint() {
        // A daemon dies and a fresh one of the same build binds the same (exclusive) endpoint. The dead
        // entry shares that endpoint, so an answer probe there hits the live successor; only the
        // owns-this-endpoint rule can drop it, and `runtrol status` must show one generation, not two.
        let scratch = Scratch::make("same-digest-restart");
        let layout = scratch.layout();
        let control = if cfg!(windows) {
            r"\\.\pipe\runtrol-generations-test-shared".to_owned()
        } else {
            scratch.0.join("shared.sock").to_string_lossy().into_owned()
        };
        let dead = RuntimeLocatorRecord {
            schema: RUNTIME_LOCATOR_SCHEMA,
            instance_id: INSTANCE.to_owned(),
            generations: vec![RuntimeGeneration {
                digest: identity(0x33).digest().to_owned(),
                endpoint_kind: endpoint_kind(),
                endpoint: "public-shared".to_owned(),
                control_endpoint: control.clone(),
                runtime_version: "0.0.0".to_owned(),
                process_id: 1,
                started_at_ms: 1,
                live_sessions: 0,
                draining: false,
            }],
        };
        write_locator(layout.runtime_locator().as_std_path(), &dead).expect("seed the dead entry");
        let live = PublishedGeneration::publish(
            &layout,
            INSTANCE,
            &identity(0x33),
            "public-shared",
            &control,
        )
        .await
        .expect("publish");
        let record = current_record(layout.runtime_locator().as_std_path())
            .expect("read")
            .expect("present");
        assert_eq!(
            record.generations.len(),
            1,
            "the dead predecessor on this daemon's own endpoint is gone"
        );
        assert_eq!(
            record.generations.first().map(|g| g.process_id),
            Some(std::process::id())
        );
        drop(live);
    }

    #[test]
    fn a_locator_from_before_generations_is_replaced_rather_than_merged() {
        let scratch = Scratch::make("legacy-locator");
        let layout = scratch.layout();
        std::fs::write(
            layout.runtime_locator().as_std_path(),
            r#"{"schema":1,"instanceId":"rtm_x","endpointKind":"namedPipe","endpoint":"x","runtimeVersion":"1","processId":1}"#,
        )
        .expect("write a schema-1 locator");
        assert!(
            current_record(layout.runtime_locator().as_std_path())
                .expect("readable")
                .is_none()
        );
    }

    #[test]
    fn the_generation_tag_is_the_first_sixteen_digits() {
        let identity = identity(0xab);
        assert_eq!(identity.tag(), "abababababababab");
        assert_eq!(identity.digest().len(), 64);
    }
}
