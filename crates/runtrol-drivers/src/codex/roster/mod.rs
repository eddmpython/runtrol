//! Which of this CLI's conversations a live process owns, and which of those have a model answering.
//!
//! # Why this is not the protocol
//!
//! This CLI multiplexes its conversations over one `app-server`, and that server answers about the threads
//! **it** has loaded. Measured 2026-08-29 against the real binary: a server this driver starts answers
//! `thread/loaded/list` with an empty list and gives every thread `status: notLoaded`, while the person's own
//! editor extension is running a turn in one of them. The protocol is per process, and the question here is
//! about processes runtrol did not start, so the answer has to come from what the CLI leaves on disk for
//! every process to see.
//!
//! # The two facts on disk, both measured on the operator's machine 2026-08-29
//!
//! **A live process owns a conversation** while it holds that conversation's writer lock,
//! `<home>/thread-writer-locks/<thread>.lock`. Of the seven locks present, the four whose conversation a live
//! process owned refused an exclusive open and the three left behind by finished processes did not. The lock
//! is released by the operating system when the process ends, so a stale file cannot claim a dead process.
//!
//! **A model is answering** while the conversation's own event log has a turn open: its last turn boundary is
//! `task_started` rather than `task_complete` or `turn_aborted`, and the opening event is not older than the
//! exact current writer process. A crash can leave an open boundary for a later resumed owner, so a writer lock
//! alone cannot make that old turn current. A process and its writer lock remain alive at
//! the CLI's paused prompt after an interruption, so the abort boundary is essential. The conversation that
//! was answering during measurement had `task_complete` at 07:20:36.672Z followed by `task_started` at
//! 07:20:36.804Z, and everything written since. This is a state, not a heartbeat, so it stays true through a
//! long tool call that writes nothing for minutes. The event timestamp only rejects a boundary from before the
//! current owner existed. Its age, file modification time, and output silence never end an established turn.
//!
//! Nothing here reads a message. The lock is a file name, and the log is read for the names of its event
//! types, their envelope timestamps, and the byte offset reached. Only the bounded structural record prefix is
//! decoded; a message body cannot supply a boundary.
//!
//! # What it costs
//!
//! One directory listing and one small open per lock, then, for each conversation a live process owns, the
//! bytes its log grew by since the last look. A conversation nobody touched costs one `metadata` call. The
//! first sight of a conversation scans backwards from the end for its last boundary, bounded, because a log
//! that has been running for days is measured in hundreds of megabytes.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::native_watch::NativeSource;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

use runtrol_provider::{
    NativeProcessActivity, NativeProcessBinding, NativeProcessObservation, NativeSessionId,
    ProcessIdentity, ProviderError, ProviderId, WallMs,
};

use crate::native_turn::NativeTurn as TurnBoundary;
#[cfg(test)]
use crate::native_turn::owner_started_at;
use crate::operator::{HomeProblem, provider_home};

mod locks;
mod missing;
use missing::MissingLogs;

/// The CLI's own override for where it keeps everything.
const HOME_ENV: &str = "CODEX_HOME";

/// The default folder under the operator's home.
const HOME_DEFAULT: &str = ".codex";

/// One lock file per conversation a process has open, named for the conversation.
const LOCKS_DIRECTORY: &str = "thread-writer-locks";

/// Where the per-conversation event logs live, under `<year>/<month>/<day>`.
const SESSIONS_DIRECTORY: &str = "sessions";
const CATALOGUE_INDEX: &str = "session_index.jsonl";

/// The extension of a lock file. The directory also holds a coordination lock, which is not a conversation.
const LOCK_EXTENSION: &str = "lock";

/// The event log's extension.
const LOG_EXTENSION: &str = "jsonl";

/// Maximum lock entries one observation will inspect.
///
/// A machine with a person at it has a handful. Walking an unbounded directory four times a second would
/// break the observation's own latency contract, so an unreasonable one is reported rather than walked.
const MAX_LOCK_ENTRIES: usize = 1024;

/// Maximum day directories searched when first placing a conversation's log.
const MAX_DAY_DIRECTORIES: usize = 400;

/// Maximum entries read inside one day directory.
const MAX_DAY_ENTRIES: usize = 4096;

/// How much of a log one backward walk reads at a time.
const SCAN_CHUNK_BYTES: u64 = 1024 * 1024;

/// How far back the first sight of a conversation walks, looking for its last turn boundary.
///
/// A turn that has written more than this since it started reads as not answering until it writes its next
/// boundary. Measured on the operator's own log, one turn wrote past four megabytes while it ran, which is
/// why this is not that number.
const MAX_BACKWARD_SCAN_BYTES: u64 = 64 * 1024 * 1024;

/// How much growth one later look will read before giving up and scanning backwards from the end instead.
const MAX_FOLLOW_BYTES: u64 = 8 * 1024 * 1024;

/// The event that opens a turn, in the CLI's own words.
const TURN_OPENED: &[u8] = b"task_started";

/// The event that closes a turn after a normal answer.
const TURN_CLOSED: &[u8] = b"task_complete";

/// The event that closes a turn after the operator or host interrupts it.
///
/// Codex keeps the conversation process and writer lock alive after this record. Treating only
/// [`TURN_CLOSED`] as a boundary therefore leaves an interrupted, idle conversation answering forever.
const TURN_ABORTED: &[u8] = b"turn_aborted";

/// The measured event envelope fits here, including its timestamp and event type. Neither a message nor the
/// remainder of a turn event is decoded. Adjacent scan chunks overlap by this bound to keep split prefixes whole.
const MAX_BOUNDARY_PREFIX_BYTES: usize = 256;

/// How long one answer about which conversations are held is reused.
///
/// Asking is an exclusive open, and an exclusive open is the one thing here that another process can notice:
/// for the moment it is held, the CLI opening that same lock would be refused. The observation clock runs
/// four times a second, so the question is asked far less often than it is answered, and a conversation that
/// opened or closed inside the last second is reported a moment late rather than probed at that rate.
const OWNERSHIP_FOR: Duration = Duration::from_secs(1);

/// What one conversation's log looked like at the last observation.
#[derive(Clone, Debug)]
struct Followed {
    /// The log this conversation writes to.
    log: PathBuf,
    /// How far the log had been read.
    read_to: u64,
    stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// The last structural boundary, kept independently of whichever process currently holds the lock.
    boundary: Option<TurnBoundary>,
    unavailable: bool,
    /// The opening record's folder, including an observed absence, at the retained file stamp.
    cwd: Option<String>,
    #[cfg(test)]
    head_reads: usize,
}

fn live_identity(pid: u32) -> Option<ProcessIdentity> {
    // A process retained by an open parent handle can still expose its birth after exit. Both facts are needed.
    if !runtrol_childproc::alive(pid) {
        return None;
    }
    runtrol_childproc::process_identity(pid)
}

/// The exact process incarnation a conversation's lock named.
///
/// Kept because asking the operating system who holds a file opens a short Restart Manager session, which is
/// far more than the four-times-a-second observation clock should pay per conversation. The answer only
/// changes when the conversation changes hands, and the holder ending is what this checks before reusing it.
#[derive(Clone, Debug)]
struct Bound {
    identity: ProcessIdentity,
}

/// Which conversations were held, and when that was asked.
#[derive(Clone, Debug)]
struct Ownership {
    threads: Vec<NativeSessionId>,
    asked_at: Instant,
}

/// The CLI's own record of which conversations are open and which are answering.
#[derive(Clone, Debug)]
pub(super) struct CodexRoster {
    home: Result<PathBuf, HomeProblem>,
    source: NativeSource,
    lock_changes: Arc<Mutex<Option<runtrol_childproc::watch::DirectoryNames>>>,
    /// The last answer about which conversations a live process holds, reused for [`Self::owned_for`].
    owned: Arc<Mutex<Option<Ownership>>>,
    /// How long that answer is reused. [`OWNERSHIP_FOR`] in the product; nothing in tests, which assert on
    /// what one call sees rather than on a clock.
    owned_for: Duration,
    /// Where each conversation's log is and how far it has been read, so a later look reads only what was
    /// appended. Shared because the driver hands clones to the blocking pool.
    followed: Arc<Mutex<HashMap<Box<str>, Followed>>>,
    missing: Arc<Mutex<MissingLogs>>,
    /// The process each conversation's lock names, and that conversation's folder. Shared for the same reason.
    bound: Arc<Mutex<HashMap<Box<str>, Bound>>>,
    /// How the holder of a lock is asked for. The product asks through a short-lived helper, which keeps the
    /// cost of that machinery out of the Runtime; a test asks in its own process, because a test binary is not
    /// a helper this executable knows how to be.
    ask_holder: fn(&Path) -> Option<ProcessIdentity>,
    /// Exact live incarnation lookup, replaceable by deterministic identities in the roster fixtures.
    identify: fn(u32) -> Option<ProcessIdentity>,
}

/// What this machine has been told about its own conversations, kept for the life of the process.
///
/// Independent preparations can build new drivers, so a driver-local invalidation cursor cannot own a
/// cache that survives those preparations. The facts below are about the machine and not about any one instance:
/// which conversations a live process holds, how far each log has been read, and which process holds which
/// lock. Asking the operating system who holds a file loads its Restart Manager, which measured 2026-08-30 as
/// two megabytes at rest and nearly five more under load when it was asked again on every observation, over a
/// budget of five for eight live sessions. Asked once per conversation, it is paid once.
#[derive(Clone, Default)]
struct MachineFacts {
    lock_changes: Arc<Mutex<Option<runtrol_childproc::watch::DirectoryNames>>>,
    owned: Arc<Mutex<Option<Ownership>>>,
    followed: Arc<Mutex<HashMap<Box<str>, Followed>>>,
    missing: Arc<Mutex<MissingLogs>>,
    bound: Arc<Mutex<HashMap<Box<str>, Bound>>>,
}

/// One current home is retained. Drivers for a previous home keep only their own facts alive.
#[derive(Default)]
#[expect(
    clippy::disallowed_types,
    reason = "synchronous construction shares one bounded cache entry without holding it across I/O or an await"
)]
struct Machine {
    facts: std::sync::Mutex<Option<(PathBuf, MachineFacts)>>,
}

impl Machine {
    fn at(&self, home: &Result<PathBuf, HomeProblem>) -> MachineFacts {
        let Ok(home) = home else {
            return MachineFacts::default();
        };
        let Ok(mut cached) = self.facts.lock() else {
            // A poisoned cache supplies no reusable ownership proof. Isolated facts remain safe.
            return MachineFacts::default();
        };
        if let Some((known, facts)) = cached.as_ref()
            && known == home
        {
            return facts.clone();
        }
        let facts = MachineFacts::default();
        let retired = cached.replace((home.clone(), facts.clone()));
        drop(cached);
        drop(retired);
        facts
    }
}

static MACHINE: std::sync::LazyLock<Machine> = std::sync::LazyLock::new(Machine::default);

impl CodexRoster {
    /// Locate the CLI's home from the environment it inherits. Opens nothing.
    #[must_use]
    pub(super) fn from_environment() -> Self {
        Self::at(provider_home(
            &mut |name| std::env::var_os(name),
            HOME_ENV,
            HOME_DEFAULT,
        ))
    }

    fn at(home: Result<PathBuf, HomeProblem>) -> Self {
        Self::holding(home, OWNERSHIP_FOR)
    }

    /// The product's roster, sharing what this process has already learned about the machine.
    fn holding(home: Result<PathBuf, HomeProblem>, owned_for: Duration) -> Self {
        Self::holding_in(home, owned_for, &MACHINE)
    }

    fn holding_in(
        home: Result<PathBuf, HomeProblem>,
        owned_for: Duration,
        machine: &Machine,
    ) -> Self {
        let facts = machine.at(&home);
        Self {
            home,
            source: NativeSource::default(),
            lock_changes: facts.lock_changes,
            owned: facts.owned,
            owned_for,
            followed: facts.followed,
            missing: facts.missing,
            bound: facts.bound,
            ask_holder: runtrol_childproc::holder_of,
            identify: live_identity,
        }
    }

    /// A roster that remembers nothing anyone else learned. Tests hold their own scratch homes and reuse the
    /// same conversation names, so sharing the machine's memory between them would let one answer another.
    #[cfg(test)]
    fn alone(home: Result<PathBuf, HomeProblem>, owned_for: Duration) -> Self {
        Self {
            home,
            source: NativeSource::default(),
            lock_changes: Arc::new(Mutex::new(None)),
            owned: Arc::new(Mutex::new(None)),
            owned_for,
            followed: Arc::new(Mutex::new(HashMap::new())),
            missing: Arc::new(Mutex::new(MissingLogs::default())),
            bound: Arc::new(Mutex::new(HashMap::new())),
            ask_holder: runtrol_childproc::holder_of_here,
            identify: live_identity,
        }
    }

    #[cfg(test)]
    fn rooted(home: PathBuf) -> Self {
        let mut roster = Self::alone(Ok(home), Duration::ZERO);
        roster.identify = tests::fixture_identity;
        roster.ask_holder = tests::fixture_holder;
        roster
    }

    /// The directory this CLI keeps one writer lock per open conversation in. Its file set changing is this
    /// CLI's own statement that a conversation was opened or closed.
    pub(super) fn locks_directory(&self) -> Option<PathBuf> {
        match &self.home {
            Ok(home) => Some(home.join(LOCKS_DIRECTORY)),
            Err(_) => None,
        }
    }

    pub(super) fn watch(
        &self,
        provider: ProviderId,
        scan: &'static crate::roster_scan::RosterScan,
    ) -> Result<Box<dyn runtrol_provider::NativeActivityWatch>, ProviderError> {
        let home = self
            .home
            .as_ref()
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing: "locating native structural observation sources",
                detail: error.to_string(),
            })?;
        let source = self.source.watch(
            provider,
            home,
            scan,
            &[LOCKS_DIRECTORY, SESSIONS_DIRECTORY, CATALOGUE_INDEX],
        )?;
        Ok(Box::new(locks::LockWatch::new(
            provider,
            source,
            Arc::clone(&self.lock_changes),
        )))
    }

    pub(super) fn observation(
        &self,
        provider: ProviderId,
    ) -> Result<NativeProcessObservation, ProviderError> {
        let activity = self.activity(provider)?;
        if activity.processes.len() != activity.live.len() {
            // Without every exact holder, there is no process completion event to wake this source.
            // Keep that failed observation recoverable instead of publishing permanent live proof.
            return Err(ProviderError::Protocol {
                provider,
                doing: "retaining current native process ownership",
                detail: "a live conversation's exact process owner is temporarily unavailable"
                    .into(),
            });
        }
        let followed = self.followed.blocking_lock();
        if followed.values().any(|known| known.unavailable) {
            return Err(ProviderError::Protocol {
                provider,
                doing: "reading current native structural metadata",
                detail: "the original session metadata is temporarily unavailable".into(),
            });
        }
        let bound = self.bound.blocking_lock();
        let mut signature = Sha256::new();
        let mut identities = Vec::new();
        let mut unknown_activity = Vec::new();
        for native in &activity.live {
            let bytes = native.as_str().as_bytes();
            signature.update((bytes.len() as u64).to_le_bytes());
            signature.update(bytes);
            let binding = bound.get(native.as_str());
            if let Some(binding) = binding {
                identities.push(binding.identity);
                signature.update(binding.identity.pid().to_le_bytes());
                signature.update(binding.identity.started().to_le_bytes());
                let folder = followed
                    .get(native.as_str())
                    .and_then(|known| known.cwd.as_deref())
                    .unwrap_or_default()
                    .as_bytes();
                signature.update((folder.len() as u64).to_le_bytes());
                signature.update(folder);
            }
            let boundary = followed
                .get(native.as_str())
                .and_then(|known| known.boundary);
            if let Some(boundary) = boundary {
                signature.update([u8::from(boundary.answering)]);
                signature.update(boundary.at.map_or(0, WallMs::as_millis).to_le_bytes());
            }
            if !binding.zip(boundary).is_some_and(|(binding, boundary)| {
                boundary.state_for(Some(binding.identity)).is_some()
            }) {
                unknown_activity.push(native.clone());
            }
        }
        // The provider's metadata-only index is append/replacement notification for names. It contains
        // no conversation body; its filesystem revision invalidates the authoritative catalogue once.
        if let Ok(home) = &self.home {
            signature.update(
                crate::native_watch::catalogue_stamp(&home.join(CATALOGUE_INDEX)).map_err(
                    |error| ProviderError::Protocol {
                        provider,
                        doing: "observing provider catalogue metadata",
                        detail: error.to_string(),
                    },
                )?,
            );
        }
        let revision = self
            .source
            .record(provider, signature.finalize().into(), identities)?;
        Ok(NativeProcessObservation {
            activity,
            catalogue_revision: Some(revision),
            unknown_activity,
        })
    }

    /// The conversations this CLI has open, and the subset with a model answering.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the lock directory exists and cannot be listed, or holds more entries
    /// than one observation may walk. A single unreadable entry is stepped over: the CLI creates and removes
    /// these while this runs.
    pub(super) fn activity(
        &self,
        provider: ProviderId,
    ) -> Result<NativeProcessActivity, ProviderError> {
        self.refresh_ownership();
        let Ok(home) = &self.home else {
            return Ok(NativeProcessActivity::default());
        };
        let owned = {
            // Taken with the blocking form on purpose: every caller reaches this from the blocking pool, and
            // the async form cannot be awaited from outside a runtime task.
            let mut cached = self.owned.blocking_lock();
            match cached.as_ref() {
                Some(known) if known.asked_at.elapsed() < self.owned_for => known.threads.clone(),
                _ => {
                    let threads = owned_threads(provider, &home.join(LOCKS_DIRECTORY))?;
                    *cached = Some(Ownership {
                        threads: threads.clone(),
                        asked_at: Instant::now(),
                    });
                    threads
                }
            }
        };
        let mut live = Vec::with_capacity(owned.len());
        let mut active = Vec::new();
        let mut processes = Vec::new();
        let locks = home.join(LOCKS_DIRECTORY);
        let mut bound = self.bound.blocking_lock();
        bound.retain(|thread, _| owned.iter().any(|owned| owned.as_str() == thread.as_ref()));
        // Conversations nobody owns any more stop being followed, so a machine that has run for a week is
        // not carrying an entry per conversation it once saw.
        // Taken with the blocking form on purpose: every caller reaches this from the blocking pool (the
        // driver's `native_process_activity` spawns it there), and the async form cannot be awaited from
        // outside a runtime task.
        let mut followed = self.followed.blocking_lock();
        followed.retain(|thread, _| owned.iter().any(|owned| owned.as_str() == thread.as_ref()));
        let mut missing = self.missing.blocking_lock();
        missing.begin(home, &owned, &followed);
        for thread in owned {
            if let Some(binding) = holder(
                self.ask_holder,
                self.identify,
                &locks,
                &mut bound,
                thread.as_str(),
            ) {
                if answering(
                    home,
                    &mut followed,
                    thread.as_str(),
                    binding.identity,
                    &mut missing,
                ) {
                    active.push(thread.clone());
                }
                processes.push(NativeProcessBinding {
                    pid: binding.identity.pid(),
                    native: thread.clone(),
                    cwd: followed
                        .get(thread.as_str())
                        .and_then(|known| known.cwd.clone()),
                    // Whether that process draws a screen another window can join is a separate question
                    // this cannot answer yet. Both surfaces of this CLI (its terminal interface and the
                    // editor extension's app server) are the same executable, so the two were told apart on
                    // this machine only by a command line, which reading from another process on Windows
                    // costs an undocumented walk of its memory. Claiming a screen that is not there made a
                    // row appear and vanish four times a second (2026-08-30, the editor's Claude panel), so
                    // nothing is claimed until a measured signal exists. Binding does not need it.
                    terminal_access: runtrol_provider::NativeTerminalAccess::Unavailable,
                });
            }
            live.push(thread);
        }
        Ok(NativeProcessActivity {
            live,
            active,
            processes,
        })
    }
}

/// The exact process holding one conversation's lock, asked once and kept.
///
/// The operating system names the user of a lock file (`runtrol_childproc::holder_of`), which is what turns
/// this CLI's per-conversation lock into a binding between a conversation and a live process. Without it a
/// terminal this Runtime started stays unbound to the conversation the person then opened inside it, and its
/// row reads as running somewhere else (operator, 2026-08-30, a conversation this Runtime was itself hosting).
///
/// A kept answer is reused while its process is still alive, so the Restart Manager session is opened once per
/// conversation rather than on every observation.
fn holder(
    ask: fn(&Path) -> Option<ProcessIdentity>,
    identify: fn(u32) -> Option<ProcessIdentity>,
    locks: &Path,
    bound: &mut HashMap<Box<str>, Bound>,
    thread: &str,
) -> Option<Bound> {
    if let Some(known) = bound.get_mut(thread)
        && identify(known.identity.pid()) == Some(known.identity)
    {
        return Some(known.clone());
    }
    bound.remove(thread);
    let reply = ask(&locks.join(format!("{thread}.{LOCK_EXTENSION}")))?;
    if identify(reply.pid()) != Some(reply) {
        return None;
    }
    let fresh = Bound { identity: reply };
    bound.insert(thread.into(), fresh.clone());
    Some(fresh)
}

/// Where a conversation works, from the first record its own log opens with.
///
/// The CLI writes that record when it creates the conversation and never rewrites it, so one bounded read of
/// the head answers for the life of the conversation. Only the folder is taken: a structural key found as
/// bytes, with no message decoded and no copy kept.
fn workspace_of(log: &Path) -> Option<String> {
    const MAX_HEAD_BYTES: usize = 64 * 1024;

    // A log that cannot be opened or read names no folder, which is the same answer a log with no folder
    // record gives: the conversation is bound to its process without one rather than filed under a guess.
    let Ok(file) = fs::File::open(log) else {
        return None;
    };
    let mut head = Vec::new();
    if BufReader::new(file)
        .take(MAX_HEAD_BYTES as u64 + 1)
        .read_until(b'\n', &mut head)
        .is_err()
        || head.len() > MAX_HEAD_BYTES
    {
        return None;
    }
    // Later records need not be valid UTF-8 at this read boundary. Only the opening record names the folder.
    let Ok(head) = std::str::from_utf8(&head) else {
        return None;
    };
    folder_in_head(head)
}

/// The folder named by one opening record, found as bytes.
fn folder_in_head(line: &str) -> Option<String> {
    const FOLDER_KEY: &str = "\"cwd\":\"";
    let start = line.find(FOLDER_KEY)? + FOLDER_KEY.len();
    let rest = line.get(start..)?;
    let end = rest.find('\"')?;
    let escaped = rest.get(..end)?;
    // The record is JSON, so a Windows path arrives with its separators escaped.
    Some(escaped.replace("\\\\", "\\"))
}

/// The conversations whose writer lock a live process is holding.
fn owned_threads(
    provider: ProviderId,
    locks: &Path,
) -> Result<Vec<NativeSessionId>, ProviderError> {
    let read_failure = |detail: std::io::Error| ProviderError::Protocol {
        provider,
        doing: "reading which conversations this CLI's live processes own",
        detail: detail.to_string(),
    };
    let entries = match fs::read_dir(locks) {
        Ok(entries) => entries,
        // The CLI has not run on this machine, or keeps its home elsewhere. Neither is a fault: both
        // mean nothing of it is open.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_failure(error)),
    };
    let mut owned = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_LOCK_ENTRIES {
            return Err(read_failure(std::io::Error::other(format!(
                "the conversation lock directory exceeds its {MAX_LOCK_ENTRIES} entry observation bound"
            ))));
        }
        let entry = entry.map_err(read_failure)?;
        let path = entry.path();
        let Some(thread) = locks::lock_name(&entry.file_name()) else {
            // The provider's writer identities are UUIDs. Its coordination lock is not a session.
            continue;
        };
        if runtrol_childproc::write_locked(&path) {
            owned.push(thread);
        }
    }
    owned.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(owned)
}

/// Whether this conversation has a turn open, reading only what its log grew by.
fn answering(
    home: &Path,
    followed: &mut HashMap<Box<str>, Followed>,
    thread: &str,
    owner: ProcessIdentity,
    missing: &mut MissingLogs,
) -> bool {
    if let Some(known) = followed.get_mut(thread) {
        let Ok(metadata) = fs::metadata(&known.log) else {
            known.boundary = None;
            known.unavailable = true;
            known.read_to = 0;
            known.cwd = None;
            return false;
        };
        let size = metadata.len();
        // File identity and write stamps invalidate metadata reads only, never model state.
        let stamp = crate::native_watch::source_times(&metadata);
        let replaced = stamp.0 != known.stamp.0 || (size == known.read_to && stamp != known.stamp);
        if size == known.read_to && !replaced && !known.unavailable {
            return known
                .boundary
                .is_some_and(|boundary| boundary.answers_for(owner));
        }
        // Location discovery and header reads share this source's stamp. A missing cwd is a
        // cached header fact, not permission to repeat a directory walk on every owner-cache hit.
        known.cwd = workspace_of(&known.log);
        #[cfg(test)]
        {
            known.head_reads += 1;
        }
        if replaced || size < known.read_to || size - known.read_to > MAX_FOLLOW_BYTES {
            // Truncated, replaced, or grown further than one look may read. Ask the file again from the
            // end rather than trusting an offset into a file that is no longer the same one.
            let Some(fresh) = last_boundary(&known.log) else {
                known.boundary = None;
                known.unavailable = true;
                known.read_to = 0;
                return false;
            };
            known.stamp = stamp;
            known.unavailable = false;
            known.read_to = fresh.0;
            known.boundary = fresh.1;
            return known
                .boundary
                .is_some_and(|boundary| boundary.answers_for(owner));
        }
        // Back over the straddle: the previous size may have landed inside a boundary token, and a token
        // that neither look sees is a turn that never starts or never ends until the next one.
        let straddle = MAX_BOUNDARY_PREFIX_BYTES as u64;
        let from = known.read_to.saturating_sub(straddle);
        match checked_boundary_in_range(&known.log, from, size) {
            Ok(Some(state)) => known.boundary = Some(state),
            Ok(None) => {}
            Err(()) => {
                known.boundary = None;
                known.unavailable = true;
                known.read_to = 0;
                return false;
            }
        }
        known.stamp = stamp;
        known.unavailable = false;
        known.read_to = size;
        return known
            .boundary
            .is_some_and(|boundary| boundary.answers_for(owner));
    }
    let Some(log) = missing.locate(home, thread) else {
        return false;
    };
    let (read_to, boundary, unavailable) = match last_boundary(&log) {
        Some((read_to, boundary)) => (read_to, boundary, false),
        None => (0, None, true),
    };
    let metadata = fs::metadata(&log);
    let unavailable = unavailable || metadata.is_err();
    // Preserve the located path on failure so the producer retries it without another directory walk.
    let stamp = metadata.map_or((None, None), |metadata| {
        crate::native_watch::source_times(&metadata)
    });
    let boundary = if unavailable { None } else { boundary };
    followed.insert(
        thread.into(),
        Followed {
            cwd: workspace_of(&log),
            #[cfg(test)]
            head_reads: 1,
            log,
            read_to,
            stamp,
            boundary,
            unavailable,
        },
    );
    boundary.is_some_and(|boundary| boundary.answers_for(owner))
}

/// The log file this conversation writes to.
///
/// The CLI names it `rollout-<timestamp>-<thread>.jsonl` under a day directory, so the search is for a
/// suffix rather than a guess at the timestamp. Bounded, and done once per conversation: the answer is kept
/// for as long as a live process owns it.
#[cfg(test)]
fn locate_log(home: &Path, thread: &str) -> Option<PathBuf> {
    search_log(home, thread).0
}

/// An absence can be retained only after a complete, readable tree with no redirected subdirectories.
fn search_log(home: &Path, thread: &str) -> (Option<PathBuf>, bool) {
    let suffix = format!("-{thread}.{LOG_EXTENSION}");
    let sessions = home.join(SESSIONS_DIRECTORY);
    let mut days = Vec::new();
    let mut complete =
        fs::symlink_metadata(&sessions).is_ok_and(|metadata| !metadata.file_type().is_symlink());
    complete &= collect_day_directories(&sessions, 0, &mut days);
    // Newest first: a conversation a process is holding was written to recently, and the day directories
    // sort by name into calendar order.
    days.sort_by(|left, right| right.cmp(left));
    for day in days.into_iter().take(MAX_DAY_DIRECTORIES) {
        let Ok(entries) = fs::read_dir(&day) else {
            complete = false;
            continue;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DAY_ENTRIES {
                complete = false;
                break;
            }
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
            {
                return (Some(path), complete);
            }
        }
    }
    (None, complete)
}

/// Every `<year>/<month>/<day>` directory under the sessions root, three levels down.
fn collect_day_directories(at: &Path, depth: usize, into: &mut Vec<PathBuf>) -> bool {
    if depth == 3 {
        into.push(at.to_path_buf());
        return true;
    }
    let Ok(entries) = fs::read_dir(at) else {
        return false;
    };
    let mut complete = true;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DAY_ENTRIES || into.len() >= MAX_DAY_DIRECTORIES {
            return false;
        }
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        match entry.file_type() {
            Ok(kind) if !kind.is_symlink() => {}
            // A subtree notification follows directory names, not a symbolic link's remote target.
            Ok(_) | Err(_) => complete = false,
        }
        let path = entry.path();
        if path.is_dir() {
            complete &= collect_day_directories(&path, depth + 1, into);
        }
    }
    complete
}

/// The state of the last turn boundary in the log, and the size the answer was read at.
///
/// Walks backwards from the end one chunk at a time and stops at the first chunk holding a boundary, which is
/// the last boundary in the file. A whole turn of this CLI is megabytes of tool calls and reasoning, measured
/// on the operator's own log: a single window sized for a comfortable read found no boundary at all in a turn
/// that had been running for half an hour, and reported it as finished. Chunks bound the memory, and
/// [`MAX_BACKWARD_SCAN_BYTES`] bounds the work.
///
/// A log whose last boundary is further back than that reads as no open turn: a conversation turns only on
/// proof that a turn is open.
fn last_boundary(log: &Path) -> Option<(u64, Option<TurnBoundary>)> {
    let Ok(mut file) = fs::File::open(log) else {
        return None;
    };
    let Ok(metadata) = file.metadata() else {
        return None;
    };
    let size = metadata.len();
    let floor = size.saturating_sub(MAX_BACKWARD_SCAN_BYTES);
    // A boundary written across a chunk edge belongs to neither chunk alone, so each chunk carries the first
    // bytes of the one after it.
    let straddle = MAX_BOUNDARY_PREFIX_BYTES as u64;
    let mut end = size;
    while end > floor {
        let start = end.saturating_sub(SCAN_CHUNK_BYTES).max(floor);
        let stop = (end + straddle).min(size);
        let bytes = read_range(&mut file, start.saturating_sub(1), stop)?;
        if let Some(boundary) = boundary_in_read_range(&bytes, start == 0) {
            return Some((size, Some(boundary)));
        }
        end = start;
    }
    Some((size, None))
}

/// The state of the last boundary inside one range of the log, or `None` when the range holds none.
#[cfg(test)]
fn boundary_in_range(log: &Path, from: u64, to: u64) -> Option<TurnBoundary> {
    checked_boundary_in_range(log, from, to).unwrap_or_default()
}

fn checked_boundary_in_range(log: &Path, from: u64, to: u64) -> Result<Option<TurnBoundary>, ()> {
    let mut file = fs::File::open(log).map_err(|_| ())?;
    let bytes = read_range(&mut file, from.saturating_sub(1), to).ok_or(())?;
    Ok(boundary_in_read_range(&bytes, from == 0))
}

/// A nonzero read starts one byte before the requested range. Its first complete line begins after the first LF:
/// that may be the preceding byte itself, or the end of a partial record. Without this proof, a nested object
/// exactly aligned with a chunk start could impersonate the provider's top-level event envelope.
fn boundary_in_read_range(bytes: &[u8], starts_file: bool) -> Option<TurnBoundary> {
    let complete = if starts_file {
        bytes
    } else {
        let newline = bytes.iter().position(|byte| *byte == b'\n')?;
        bytes.get(newline + 1..)?
    };
    last_turn_boundary(complete)
}

/// One range of an open log, or `None` for anything the filesystem refuses.
///
/// A read that did not happen must not be turned into a state here; the caller decides what an unread log
/// means where that is decided.
fn read_range(file: &mut fs::File, from: u64, to: u64) -> Option<Vec<u8>> {
    if to < from {
        return None;
    }
    if file.seek(SeekFrom::Start(from)).is_err() {
        return None;
    }
    let Ok(length) = usize::try_from(to - from) else {
        return None;
    };
    let mut bytes = vec![0_u8; length];
    if file.read_exact(&mut bytes).is_err() {
        return None;
    }
    Some(bytes)
}

/// Whether the last turn boundary in these bytes opens a turn, or `None` when they hold no boundary.
fn last_turn_boundary(bytes: &[u8]) -> Option<TurnBoundary> {
    bytes.rsplit(|byte| *byte == b'\n').find_map(turn_boundary)
}

/// Read only the structural envelope, independent of JSON whitespace and object field order. Root metadata
/// and the payload's direct type must fit the existing prefix bound. Nothing after those facts is decoded.
fn turn_boundary(line: &[u8]) -> Option<TurnBoundary> {
    let prefix = line.get(..line.len().min(MAX_BOUNDARY_PREFIX_BYTES))?;
    let mut cursor = EnvelopeCursor(prefix);
    let mut envelope = BoundaryEnvelope::default();
    cursor.take(b'{')?;
    loop {
        match cursor.key()?.as_str() {
            "timestamp" => {
                // An unreadable timestamp overrides an earlier open turn with an unknown boundary.
                envelope.at =
                    EnvelopeTimestamp::Present(WallMs::from_iso8601(&cursor.value::<String>()?));
            }
            "type" => {
                if cursor.value::<String>()? != "event_msg" {
                    return None;
                }
                envelope.event = true;
            }
            "payload" => {
                cursor.payload(&mut envelope)?;
                if let Some(boundary) = envelope.boundary() {
                    return Some(boundary);
                }
            }
            _ => cursor.ignore()?,
        }
        let more = cursor.next_member()?;
        if let Some(boundary) = envelope.boundary() {
            return Some(boundary);
        }
        if !more {
            return None;
        }
    }
}

/// These three structural facts are the only values retained from a record prefix.
#[derive(Default)]
struct BoundaryEnvelope {
    event: bool,
    at: EnvelopeTimestamp,
    answering: Option<bool>,
}

#[derive(Clone, Copy, Default)]
enum EnvelopeTimestamp {
    #[default]
    Missing,
    Present(Option<WallMs>),
}

impl BoundaryEnvelope {
    fn boundary(&self) -> Option<TurnBoundary> {
        if !self.event {
            return None;
        }
        let EnvelopeTimestamp::Present(at) = self.at else {
            return None;
        };
        Some(TurnBoundary {
            answering: self.answering?,
            at,
        })
    }
}

/// A bounded cursor through exactly the root object and its payload object. Serde skips unknown values
/// iteratively without materializing them; its temporary nesting scratch cannot outgrow this byte prefix.
struct EnvelopeCursor<'a>(&'a [u8]);

impl<'a> EnvelopeCursor<'a> {
    fn whitespace(&mut self) {
        while let Some((b' ' | b'\t' | b'\r' | b'\n', remaining)) = self.0.split_first() {
            self.0 = remaining;
        }
    }

    fn take(&mut self, expected: u8) -> Option<()> {
        self.whitespace();
        let (byte, remaining) = self.0.split_first()?;
        if *byte != expected {
            return None;
        }
        self.0 = remaining;
        Some(())
    }

    fn value<T: serde::Deserialize<'a>>(&mut self) -> Option<T> {
        let mut stream = serde_json::Deserializer::from_slice(self.0).into_iter::<T>();
        let Ok(value) = stream.next()? else {
            return None;
        };
        self.0 = self.0.get(stream.byte_offset()..)?;
        Some(value)
    }

    fn key(&mut self) -> Option<String> {
        let key = self.value()?;
        self.take(b':')?;
        Some(key)
    }

    fn ignore(&mut self) -> Option<()> {
        self.value::<serde::de::IgnoredAny>()?;
        Some(())
    }

    /// Require a complete scalar and its enclosing delimiter before accepting a boundary.
    fn next_member(&mut self) -> Option<bool> {
        self.whitespace();
        let (byte, remaining) = self.0.split_first()?;
        let more = match byte {
            b',' => true,
            b'}' => false,
            _ => return None,
        };
        self.0 = remaining;
        Some(more)
    }

    fn payload(&mut self, envelope: &mut BoundaryEnvelope) -> Option<()> {
        self.take(b'{')?;
        loop {
            if self.key()? == "type" {
                envelope.answering = match self.value::<String>()?.as_bytes() {
                    TURN_OPENED => Some(true),
                    TURN_CLOSED | TURN_ABORTED => Some(false),
                    _ => return None,
                };
            } else {
                self.ignore()?;
            }
            let more = self.next_member()?;
            if envelope.boundary().is_some() || !more {
                return Some(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use std::fs::OpenOptions;
    #[cfg(windows)]
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    pub(super) struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn codex() -> ProviderId {
        ProviderId::parse("codex").expect("the built-in provider identity parses")
    }

    pub(super) const OPEN_THREAD: &str = "01a03afd-5184-7512-8b48-b77dd957d18a";
    #[cfg(windows)]
    const DONE_THREAD: &str = "01a0471d-786c-7561-a8d5-db5ddb837c0c";

    const TURN_AT: &str = "2026-08-29T00:00:01.000Z";

    #[test]
    fn a_recycled_pid_in_the_holder_reply_cannot_create_a_binding() {
        let mut bound = HashMap::new();
        let result = holder(
            |_| ProcessIdentity::new(42, 100),
            |pid| ProcessIdentity::new(pid, 200),
            Path::new("unused-locks"),
            &mut bound,
            OPEN_THREAD,
        );
        assert!(
            result.is_none(),
            "the queried holder was an older incarnation of this PID"
        );
        assert!(
            bound.is_empty(),
            "the replacement must not inherit the old file ownership"
        );
    }

    pub(super) fn fixture_holder(path: &Path) -> Option<ProcessIdentity> {
        // Only test turn timestamps are synthetic; the operating system must still name this fixture's holder.
        runtrol_childproc::holder_of_here(path)
            .and_then(|identity| fixture_identity(identity.pid()))
    }

    #[cfg(windows)]
    fn resumed_holder(path: &Path) -> Option<ProcessIdentity> {
        runtrol_childproc::holder_of_here(path)
            .and_then(|identity| resumed_identity(identity.pid()))
    }

    pub(super) fn fixture_identity(pid: u32) -> Option<ProcessIdentity> {
        // Fixed FILETIME for 2026-08-29T00:00:00Z. The synthetic turn begins one second later.
        ProcessIdentity::new(pid, 134_324_352_000_000_000)
    }

    #[cfg(windows)]
    fn resumed_identity(pid: u32) -> Option<ProcessIdentity> {
        // The same PID, reincarnated two seconds later, after the fixture turn began.
        ProcessIdentity::new(pid, 134_324_352_020_000_000)
    }

    fn last_open_turn(bytes: &[u8]) -> Option<bool> {
        last_turn_boundary(bytes).map(|boundary| boundary.answering)
    }

    fn chunk_bytes() -> usize {
        usize::try_from(SCAN_CHUNK_BYTES).expect("the bounded scan chunk fits the test platform")
    }

    pub(super) fn opened(id: &str) -> String {
        opened_at(id, TURN_AT)
    }

    fn opened_at(id: &str, at: &str) -> String {
        format!(
            "{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"{id}\"}}}}\n"
        )
    }

    fn closed(id: &str) -> String {
        format!(
            "{{\"timestamp\":\"{TURN_AT}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"turn_id\":\"{id}\"}}}}\n"
        )
    }

    fn aborted(id: &str) -> String {
        format!(
            "{{\"timestamp\":\"{TURN_AT}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"turn_aborted\",\"turn_id\":\"{id}\"}}}}\n"
        )
    }

    fn chatter() -> String {
        "{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{}}}\n"
            .to_owned()
    }

    /// A home shaped the way the CLI shapes its own, with the given conversations and logs.
    pub(super) fn home(logs: &[(&str, String)]) -> (Scratch, PathBuf) {
        let serial = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "runtrol-codex-roster-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(LOCKS_DIRECTORY)).expect("the lock directory is created");
        let day = root
            .join(SESSIONS_DIRECTORY)
            .join("2026")
            .join("08")
            .join("29");
        fs::create_dir_all(&day).expect("the day directory is created");
        for (thread, body) in logs {
            fs::write(
                day.join(format!("rollout-2026-08-29T00-00-00-{thread}.jsonl")),
                body,
            )
            .expect("a conversation log is written");
            fs::write(
                root.join(LOCKS_DIRECTORY).join(format!("{thread}.lock")),
                b"",
            )
            .expect("a conversation lock is written");
        }
        (Scratch(root.clone()), root)
    }

    /// Hold a conversation's lock the way a live process of the CLI holds it.
    #[cfg(windows)]
    fn hold(root: &Path, thread: &str) -> fs::File {
        use std::os::windows::fs::OpenOptionsExt as _;
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(root.join(LOCKS_DIRECTORY).join(format!("{thread}.lock")))
            .expect("the conversation lock is taken")
    }

    #[test]
    fn a_home_that_cannot_be_named_reports_nothing_rather_than_failing() {
        let roster = CodexRoster::at(Err(HomeProblem::Missing));
        let activity = roster
            .activity(codex())
            .expect("an unknown home is not a fault");
        assert!(activity.live.is_empty());
        assert!(activity.active.is_empty());
    }

    #[test]
    fn a_lock_nobody_holds_names_no_conversation() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("turn") + &chatter())]);
        let roster = CodexRoster::rooted(root);
        let activity = roster.activity(codex()).expect("the home is readable");
        assert!(
            activity.live.is_empty(),
            "a lock file left behind by a finished process owns nothing"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_resumed_owner_does_not_inherit_an_unfinished_turn() {
        let old_turn = opened_at("old", "2020-01-01T00:00:00Z");
        let (_kept, root) = home(&[(OPEN_THREAD, old_turn)]);
        let _held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root);
        let activity = roster.activity(codex()).expect("the home is readable");
        assert_eq!(activity.live.len(), 1, "the resumed owner remains live");
        assert!(
            activity.active.is_empty(),
            "the new owner cannot inherit a turn opened before that process existed"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_recycled_holder_pid_cannot_keep_a_cached_turn_active() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("old"))]);
        let _held = hold(&root, OPEN_THREAD);
        let mut roster = CodexRoster::rooted(root);
        let first = roster.activity(codex()).expect("the home is readable");
        assert_eq!(first.active.len(), 1, "the original owner opened this turn");

        roster.identify = resumed_identity;
        roster.ask_holder = resumed_holder;
        let second = roster.activity(codex()).expect("the home is readable");
        assert_eq!(second.live.len(), 1, "the replacement holder stays live");
        assert!(
            second.active.is_empty(),
            "cached bytes belong to the old owner"
        );
        assert_eq!(
            roster
                .bound
                .blocking_lock()
                .get(OPEN_THREAD)
                .expect("the replacement is cached")
                .identity,
            resumed_identity(std::process::id()).expect("the replacement has an identity")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_silent_turn_stays_active_for_the_same_exact_owner() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("only"))]);
        let _held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root);
        for _ in 0..3 {
            let activity = roster.activity(codex()).expect("the home is readable");
            assert_eq!(activity.active.len(), 1, "no newer output is required");
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_unknown_holder_identity_keeps_ownership_without_claiming_work() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("only"))]);
        let _held = hold(&root, OPEN_THREAD);
        let mut roster = CodexRoster::rooted(root);
        assert_eq!(roster.activity(codex()).expect("readable").active.len(), 1);
        roster.identify = |_pid| None;
        let activity = roster.activity(codex()).expect("the home is readable");
        assert_eq!(activity.live.len(), 1);
        assert!(activity.active.is_empty());
        assert!(activity.processes.is_empty());
        assert!(roster.bound.blocking_lock().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_kernel_birth_uses_the_same_wall_epoch_as_the_structural_boundary() {
        let owner = live_identity(std::process::id()).expect("the test process is live");
        let born = owner_started_at(owner).expect("Windows publishes an absolute birth");
        assert!(born <= WallMs::now());
        let before = born
            .as_millis()
            .checked_sub(1)
            .expect("the process started after the epoch");
        assert!(
            !TurnBoundary {
                answering: true,
                at: Some(WallMs::from_millis(before))
            }
            .answers_for(owner)
        );
        assert!(
            TurnBoundary {
                answering: true,
                at: Some(WallMs::now())
            }
            .answers_for(owner)
        );
        assert_eq!(
            owner_started_at(fixture_identity(1).expect("fixture identity")),
            WallMs::from_iso8601("2026-08-29T00:00:00Z")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn an_opaque_non_windows_birth_is_not_compared_with_wall_time() {
        let owner = fixture_identity(1).expect("fixture identity");
        assert_eq!(owner_started_at(owner), None);
        assert!(
            !TurnBoundary {
                answering: true,
                at: Some(WallMs::now())
            }
            .answers_for(owner)
        );
    }

    /// The whole claim, in the shape it was measured in: the conversation a live process holds and whose log
    /// has a turn open is answering; the one whose log ended on a completed turn is open but not answering.
    #[cfg(windows)]
    #[test]
    fn a_held_conversation_answers_only_while_its_last_turn_boundary_is_open() {
        let (_kept, root) = home(&[
            (
                OPEN_THREAD,
                closed("first") + &opened("second") + &chatter(),
            ),
            (DONE_THREAD, opened("only") + &chatter() + &closed("only")),
        ]);
        let _open = hold(&root, OPEN_THREAD);
        let _done = hold(&root, DONE_THREAD);
        let roster = CodexRoster::rooted(root);
        let activity = roster.activity(codex()).expect("the home is readable");
        let live: Vec<&str> = activity.live.iter().map(NativeSessionId::as_str).collect();
        let active: Vec<&str> = activity
            .active
            .iter()
            .map(NativeSessionId::as_str)
            .collect();
        assert_eq!(live, vec![OPEN_THREAD, DONE_THREAD]);
        assert_eq!(active, vec![OPEN_THREAD]);
        // The lock names its holder, and in this test that holder is this process. Binding a live
        // conversation to the process that owns it is what lets a row find the terminal it belongs to.
        let bound: Vec<(u32, &str)> = activity
            .processes
            .iter()
            .map(|process| (process.pid, process.native.as_str()))
            .collect();
        assert_eq!(
            bound,
            vec![
                (std::process::id(), OPEN_THREAD),
                (std::process::id(), DONE_THREAD)
            ]
        );
        // A screen to join is a claim this cannot make yet, so it makes none.
        assert!(activity.processes.iter().all(|process| {
            matches!(
                &process.terminal_access,
                runtrol_provider::NativeTerminalAccess::Unavailable
            )
        }));
    }

    /// The second look reads only what was appended, and the turn ending there ends the answer.
    #[cfg(windows)]
    #[test]
    fn a_turn_that_ends_between_two_looks_stops_answering() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("turn") + &chatter())]);
        let held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root.clone());
        let first = roster.activity(codex()).expect("the home is readable");
        assert_eq!(first.active.len(), 1, "the turn is open");

        let log = root
            .join(SESSIONS_DIRECTORY)
            .join("2026")
            .join("08")
            .join("29")
            .join(format!("rollout-2026-08-29T00-00-00-{OPEN_THREAD}.jsonl"));
        let mut appended = OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("the log is appended to");
        appended
            .write_all(closed("turn").as_bytes())
            .expect("the turn ends");
        drop(appended);

        let second = roster.activity(codex()).expect("the home is readable");
        assert!(
            second.active.is_empty(),
            "the completed turn stops the answer"
        );
        assert_eq!(
            second.live.len(),
            1,
            "the process still owns the conversation"
        );

        drop(held);
        let third = roster.activity(codex()).expect("the home is readable");
        assert!(
            third.live.is_empty(),
            "releasing the lock is the process ending, and it owns nothing after that"
        );
    }

    /// An interrupted turn closes the working state without releasing the live conversation.
    ///
    /// Codex emits `turn_aborted`, not `task_complete`, when a host interrupts a turn. The 0.1.41 roster
    /// missed that boundary and left the sidebar glyph spinning until the whole CLI process exited.
    #[cfg(windows)]
    #[test]
    fn a_turn_that_is_aborted_between_two_looks_stops_answering() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("turn") + &chatter())]);
        let _held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root.clone());
        let first = roster.activity(codex()).expect("the home is readable");
        assert_eq!(first.active.len(), 1, "the turn is open");

        let log = root
            .join(SESSIONS_DIRECTORY)
            .join("2026")
            .join("08")
            .join("29")
            .join(format!("rollout-2026-08-29T00-00-00-{OPEN_THREAD}.jsonl"));
        let mut appended = OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("the log is appended to");
        appended
            .write_all(aborted("turn").as_bytes())
            .expect("the turn is interrupted");
        drop(appended);

        let second = roster.activity(codex()).expect("the home is readable");
        assert!(
            second.active.is_empty(),
            "the interrupted turn stops the answer"
        );
        assert_eq!(
            second.live.len(),
            1,
            "the process still owns the conversation"
        );
    }

    /// What this reads on the machine it is run on, printed rather than asserted.
    ///
    /// Ignored by default because it depends on whether the person has the CLI open right now. It is how the
    /// claim in this module's notes was checked against the real thing, and how it is checked again after the
    /// CLI changes where it keeps its locks or what it calls its turn events:
    /// `cargo test -p runtrol-drivers --lib codex::roster::tests::this_machine -- --ignored --nocapture`.
    #[ignore = "reads the operator's own CLI home"]
    #[test]
    fn this_machine_reports_what_the_cli_has_open() {
        let roster = CodexRoster::from_environment();
        let activity = roster
            .activity(codex())
            .expect("the CLI home is readable on this machine");
        println!("open conversations: {}", activity.live.len());
        for thread in &activity.live {
            let answering = activity.active.contains(thread);
            let bound = activity
                .processes
                .iter()
                .find(|process| process.native == *thread);
            let held = bound.map_or_else(
                || "unbound".to_owned(),
                |process| {
                    format!(
                        "pid={} cwd={}",
                        process.pid,
                        process.cwd.as_deref().unwrap_or("unknown")
                    )
                },
            );
            println!("  {} answering={answering} {held}", thread.as_str());
        }
    }

    #[test]
    fn the_last_boundary_in_a_window_decides_and_no_boundary_decides_nothing() {
        assert_eq!(last_open_turn(chatter().as_bytes()), None);
        assert_eq!(last_open_turn(opened("a").as_bytes()), Some(true));
        assert_eq!(last_open_turn(closed("a").as_bytes()), Some(false));
        assert_eq!(last_open_turn(aborted("a").as_bytes()), Some(false));
        assert_eq!(
            last_open_turn((closed("a") + &opened("b")).as_bytes()),
            Some(true)
        );
        assert_eq!(
            last_open_turn((opened("a") + &chatter() + &closed("a")).as_bytes()),
            Some(false)
        );
        assert_eq!(
            last_open_turn((closed("a") + &opened("b") + &aborted("b")).as_bytes()),
            Some(false)
        );
    }

    #[test]
    fn structural_envelopes_accept_whitespace_and_both_objects_field_orders() {
        for (event, answering) in [
            ("task_started", true),
            ("task_complete", false),
            ("turn_aborted", false),
        ] {
            for spaced in [false, true] {
                for payload_order in [false, true] {
                    let separator = if spaced { " : " } else { ":" };
                    let delimiter = if spaced { " ,\t" } else { "," };
                    let payload_type = format!("\"type\"{separator}\"{event}\"");
                    let opaque = format!("\"turn_id\"{separator}\"synthetic\"");
                    let payload = if payload_order {
                        format!("{opaque}{delimiter}{payload_type}")
                    } else {
                        format!("{payload_type}{delimiter}{opaque}")
                    };
                    let fields = [
                        format!("\"timestamp\"{separator}\"{TURN_AT}\""),
                        format!("\"type\"{separator}\"event_msg\""),
                        format!("\"payload\"{separator}{{ {payload} }}"),
                    ];
                    for order in [
                        [0, 1, 2],
                        [0, 2, 1],
                        [1, 0, 2],
                        [1, 2, 0],
                        [2, 0, 1],
                        [2, 1, 0],
                    ] {
                        let ordered = order.map(|index| fields.get(index).expect("field index"));
                        let line =
                            format!(" {{ {} }}\r\n", ordered.map(String::as_str).join(delimiter));
                        assert!(line.len() < MAX_BOUNDARY_PREFIX_BYTES);
                        assert_eq!(
                            last_turn_boundary(line.as_bytes()),
                            Some(TurnBoundary {
                                answering,
                                at: WallMs::from_iso8601(TURN_AT),
                            }),
                            "event={event} spaced={spaced} payload_order={payload_order} root_order={order:?}"
                        );
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_resumed_owner_sees_its_new_reordered_turn_in_warm_and_cold_rosters() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("old_owner"))]);
        let _held = hold(&root, OPEN_THREAD);
        let mut warm = CodexRoster::rooted(root.clone());
        assert_eq!(warm.activity(codex()).expect("readable").active.len(), 1);
        warm.identify = resumed_identity;
        warm.ask_holder = resumed_holder;
        assert!(warm.activity(codex()).expect("readable").active.is_empty());
        let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
        let next = concat!(
            "{ \"payload\" : { \"turn_id\" : \"new_owner\", \"type\" : \"task_started\" }, ",
            "\"type\" : \"event_msg\", \"timestamp\" : \"2026-08-29T00:00:03Z\" }\n"
        );
        OpenOptions::new()
            .append(true)
            .open(log)
            .expect("append fixture")
            .write_all(next.as_bytes())
            .expect("the new owner opens its turn");
        assert_eq!(warm.activity(codex()).expect("readable").active.len(), 1);
        let mut cold = CodexRoster::rooted(root);
        cold.identify = resumed_identity;
        cold.ask_holder = resumed_holder;
        assert_eq!(cold.activity(codex()).expect("readable").active.len(), 1);
    }

    #[test]
    fn reordered_metadata_never_uses_nested_or_quoted_event_types() {
        for payload in [
            r#"{ "body": { "type": "task_started" }, "type": "other" }"#,
            r#"{ "body": "\"type\":\"task_started\"", "type": "other" }"#,
            r#"{ "body": { "type": "task_started" } }"#,
        ] {
            let line = format!(
                "{{ \"payload\" : {payload}, \"timestamp\" : \"{TURN_AT}\", \"type\" : \"event_msg\" }}\n"
            );
            assert_eq!(last_turn_boundary(line.as_bytes()), None);
        }
        let line = format!(
            "{{ \"payload\" : {{ \"type\" : \"task_started\" }}, \"timestamp\" : \"{TURN_AT}\", \"type\" : \"response_item\" }}\n"
        );
        assert_eq!(last_turn_boundary(line.as_bytes()), None);
    }

    #[test]
    fn envelope_metadata_must_fit_the_existing_bound_without_reading_trailing_body() {
        let body = "x".repeat(MAX_BOUNDARY_PREFIX_BYTES);
        let metadata_last = format!(
            "{{\"body\":\"{body}\",\"timestamp\":\"{TURN_AT}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n"
        );
        assert_eq!(last_turn_boundary(metadata_last.as_bytes()), None);
        let metadata_first = format!(
            "{{ \"type\" : \"event_msg\", \"timestamp\" : \"{TURN_AT}\", \"payload\" : {{ \"type\" : \"task_started\", \"body\" : \"{body}\" }} }}\n"
        );
        assert_eq!(last_open_turn(metadata_first.as_bytes()), Some(true));
    }

    #[test]
    fn a_body_with_a_turn_named_type_is_not_a_boundary() {
        let body = concat!(
            "{\"timestamp\":\"2026-08-29T00:00:01Z\",\"type\":\"response_item\",",
            "\"payload\":{\"type\":\"task_started\"}}\n"
        );
        assert_eq!(last_open_turn(body.as_bytes()), None);
        let body = serde_json::to_string(&serde_json::json!({
            "timestamp": TURN_AT,
            "type": "response_item",
            "payload": {"text": opened("quoted")}
        }))
        .expect("the synthetic message is JSON");
        assert_eq!(
            last_open_turn((closed("real") + &body).as_bytes()),
            Some(false)
        );
        let nested = format!(
            "{{\"timestamp\":\"{TURN_AT}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"other\",\"body\":{{\"type\":\"task_started\"}}}}}}\n"
        );
        assert_eq!(last_open_turn(nested.as_bytes()), None);
    }

    #[cfg(windows)]
    #[test]
    fn an_unreadable_opening_time_cannot_reuse_an_earlier_current_turn() {
        let bytes = opened("valid") + &opened_at("unknown", "unavailable");
        let boundary =
            last_turn_boundary(bytes.as_bytes()).expect("the last event is a turn start");
        assert!(boundary.answering);
        assert_eq!(boundary.at, None);
        assert!(!boundary.answers_for(fixture_identity(1).expect("fixture owner")));
    }

    #[cfg(windows)]
    #[test]
    fn a_boundary_prefix_split_between_observations_is_seen_once_complete() {
        for split in [10, 80] {
            let complete = opened("split");
            let (_kept, root) = home(&[(OPEN_THREAD, complete[..split].to_owned())]);
            let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
            let owner = fixture_identity(1).expect("fixture owner");
            let mut followed = HashMap::new();
            assert!(!answering(
                &root,
                &mut followed,
                OPEN_THREAD,
                owner,
                &mut MissingLogs::default()
            ));
            let mut file = OpenOptions::new()
                .append(true)
                .open(log)
                .expect("append fixture");
            file.write_all(
                complete
                    .as_bytes()
                    .get(split..)
                    .expect("fixture split is in range"),
            )
            .expect("finish the prefix");
            assert!(answering(
                &root,
                &mut followed,
                OPEN_THREAD,
                owner,
                &mut MissingLogs::default()
            ));
        }
    }

    #[test]
    fn a_boundary_prefix_split_across_backward_chunks_is_preserved() {
        let mut body = opened("long_tool");
        // The tail has no structural boundary. Its length puts the first backward chunk inside the event prefix.
        body.extend(std::iter::repeat_n('x', chunk_bytes() - 20));
        let (_kept, root) = home(&[(OPEN_THREAD, body)]);
        let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
        let (_size, boundary) = last_boundary(&log).expect("the fixture is readable");
        assert_eq!(
            boundary,
            Some(TurnBoundary {
                answering: true,
                at: WallMs::from_iso8601(TURN_AT)
            })
        );
    }

    fn nested_turn_record(padding: usize) -> (usize, String) {
        let prefix = format!(
            "{{\"timestamp\":\"{TURN_AT}\",\"type\":\"response_item\",\"payload\":{{\"embedded\":"
        );
        let body = format!(
            "{prefix}{},\"padding\":\"{}\"}}}}\n",
            opened("nested").trim_end(),
            "x".repeat(padding)
        );
        (prefix.len(), body)
    }

    #[test]
    fn a_nested_object_at_the_backward_chunk_start_is_not_a_turn() {
        let (prefix, initial) = nested_turn_record(0);
        let padding = chunk_bytes() - (initial.len() - prefix);
        let (prefix, body) = nested_turn_record(padding);
        assert_eq!(body.len() - chunk_bytes(), prefix);
        let (_kept, root) = home(&[(OPEN_THREAD, body)]);
        let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
        assert_eq!(last_boundary(&log).expect("readable fixture").1, None);
    }

    #[cfg(windows)]
    #[test]
    fn a_nested_object_at_the_follow_start_is_not_a_turn() {
        let (prefix, body) = nested_turn_record(512);
        let observed = prefix + MAX_BOUNDARY_PREFIX_BYTES;
        let (_kept, root) = home(&[(OPEN_THREAD, body[..observed].to_owned())]);
        let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
        let owner = fixture_identity(1).expect("fixture owner");
        let mut followed = HashMap::new();
        assert!(!answering(
            &root,
            &mut followed,
            OPEN_THREAD,
            owner,
            &mut MissingLogs::default()
        ));
        let mut file = OpenOptions::new()
            .append(true)
            .open(log)
            .expect("append fixture");
        file.write_all(
            body.as_bytes()
                .get(observed..)
                .expect("fixture split is in range"),
        )
        .expect("finish the fixture");
        assert!(!answering(
            &root,
            &mut followed,
            OPEN_THREAD,
            owner,
            &mut MissingLogs::default()
        ));
    }

    #[test]
    fn a_top_level_boundary_exactly_at_the_range_start_is_kept() {
        let prefix = chatter();
        let mut body = prefix.clone() + &opened("real");
        body.extend(std::iter::repeat_n(
            'x',
            chunk_bytes() - opened("real").len(),
        ));
        assert_eq!(body.len() - chunk_bytes(), prefix.len());
        let length = body.len() as u64;
        let (_kept, root) = home(&[(OPEN_THREAD, body)]);
        let log = locate_log(&root, OPEN_THREAD).expect("the fixture log exists");
        let expected = Some(TurnBoundary {
            answering: true,
            at: WallMs::from_iso8601(TURN_AT),
        });
        assert_eq!(last_boundary(&log).expect("readable fixture").1, expected);
        assert_eq!(
            boundary_in_range(&log, prefix.len() as u64, length),
            expected
        );
    }

    #[test]
    fn the_boundary_prefix_reader_is_bounded_and_rejects_incomplete_event_names() {
        let oversized = opened_at("opaque", &"x".repeat(MAX_BOUNDARY_PREFIX_BYTES));
        assert_eq!(last_turn_boundary(oversized.as_bytes()), None);
        let incomplete = format!(
            "{{\"timestamp\":\"{TURN_AT}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\""
        );
        assert_eq!(last_turn_boundary(incomplete.as_bytes()), None);
        assert_eq!(
            last_turn_boundary(
                opened("a")
                    .replace("task_started", "task_started_other")
                    .as_bytes()
            ),
            None
        );
    }

    #[test]
    fn the_opening_record_names_the_folder_with_its_separators_restored() {
        let line = concat!(
            r#"{"timestamp":"t","type":"session_meta","payload":{"session_id":"a","#,
            r#""cwd":"c:\Users\MSI\Desktop\taxly","originator":"codex_vscode"}}"#
        );
        assert_eq!(
            folder_in_head(line).as_deref(),
            Some(r"c:\Users\MSI\Desktop\taxly")
        );
        assert_eq!(
            folder_in_head(r#"{"payload":{"cwd":"/work/app"}}"#).as_deref(),
            Some("/work/app")
        );
        // A record with no folder, and one that stops before its closing quote, name nothing rather than
        // guessing: an unknown folder files a conversation nowhere, a wrong one files it under the wrong project.
        assert_eq!(folder_in_head(r#"{"payload":{"id":"a"}}"#), None);
        assert_eq!(folder_in_head(r#"{"cwd":"unterminated"#), None);
    }

    #[test]
    fn later_multibyte_record_boundaries_cannot_hide_the_opening_folder() {
        let (_kept, root) = home(&[]);
        let log = root.join("synthetic.jsonl");
        let mut bytes =
            b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/app\"}}\n".to_vec();
        bytes.resize(64 * 1024 - 1, b' ');
        bytes.extend_from_slice("한".as_bytes());
        let original_window = bytes
            .get(..64 * 1024)
            .expect("the synthetic boundary exists");
        assert!(std::str::from_utf8(original_window).is_err());
        fs::write(&log, bytes).expect("synthetic multibyte boundary");
        assert_eq!(workspace_of(&log).as_deref(), Some("/work/app"));

        let mut oversized = vec![b' '; 64 * 1024];
        oversized.extend_from_slice(b"{\"cwd\":\"/wrong\"}\n");
        fs::write(&log, oversized).expect("oversized opening record");
        assert_eq!(workspace_of(&log), None);
        fs::write(&log, b"\xff{\"cwd\":\"/wrong\"}\n").expect("invalid opening record");
        assert_eq!(workspace_of(&log), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_temporarily_unreadable_source_requires_recovery_and_does_not_keep_turn_proof() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let (_kept, root) = home(&[(OPEN_THREAD, opened("turn"))]);
        let _held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root);
        assert_eq!(
            roster
                .observation(codex())
                .expect("first complete observation")
                .activity
                .active
                .len(),
            1
        );
        let log = roster
            .followed
            .blocking_lock()
            .get(OPEN_THREAD)
            .expect("followed source")
            .log
            .clone();
        let mut denied = OpenOptions::new()
            .read(true)
            .append(true)
            .share_mode(0)
            .open(&log)
            .expect("exclusive synthetic writer");
        std::io::Write::write_all(&mut denied, chatter().as_bytes())
            .expect("new bytes demand a current read");
        assert!(
            roster.observation(codex()).is_err(),
            "I/O failure requests bounded recovery instead of sleeping forever"
        );
        assert!(
            roster
                .followed
                .blocking_lock()
                .get(OPEN_THREAD)
                .expect("followed source")
                .boundary
                .is_none()
        );
        drop(denied);
        assert_eq!(
            roster
                .observation(codex())
                .expect("source becomes readable")
                .activity
                .active
                .len(),
            1
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_unresolved_holder_requires_recovery_before_an_event_driven_observation() {
        let (_kept, root) = home(&[(OPEN_THREAD, opened("turn"))]);
        let _held = hold(&root, OPEN_THREAD);
        let mut roster = CodexRoster::rooted(root);
        roster.ask_holder = |_| None;
        assert!(roster.observation(codex()).is_err());
        roster.ask_holder = fixture_holder;
        let recovered = roster
            .observation(codex())
            .expect("exact owner becomes available");
        assert_eq!(recovered.activity.live.len(), 1);
        assert_eq!(recovered.activity.processes.len(), 1);
        assert_eq!(recovered.activity.active.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn new_preparations_share_the_same_home_ownership_cursor_and_preserve_bindings() {
        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let _held = hold(&root, OPEN_THREAD);
        let machine = Machine::default();
        let mut first = CodexRoster::holding_in(Ok(root.clone()), Duration::ZERO, &machine);
        first.ask_holder = fixture_holder;
        first.identify = fixture_identity;
        let observed = first.activity(codex()).expect("initial owner");
        assert_eq!(observed.processes.len(), 1);
        let mut second = CodexRoster::holding_in(Ok(root), Duration::ZERO, &machine);
        second.identify = fixture_identity;
        second.ask_holder = |_| panic!("a new preparation must retain the same exact owner");
        assert!(Arc::ptr_eq(&first.lock_changes, &second.lock_changes));
        assert_eq!(
            second
                .activity(codex())
                .expect("shared owner")
                .processes
                .len(),
            1
        );

        let (_other_kept, other_root) = home(&[(OPEN_THREAD, String::new())]);
        let other = CodexRoster::holding_in(Ok(other_root), Duration::ZERO, &machine);
        assert!(!Arc::ptr_eq(&first.lock_changes, &other.lock_changes));
        assert!(other.bound.blocking_lock().is_empty());
        assert!(!first.bound.blocking_lock().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn absent_locations_and_headers_are_reused_until_their_source_changes() {
        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let log = locate_log(&root, OPEN_THREAD).expect("empty fixture log");
        fs::remove_file(&log).expect("writer can precede its log");
        let _held = hold(&root, OPEN_THREAD);
        let roster = CodexRoster::rooted(root.clone());
        roster.activity(codex()).expect("initial missing location");
        let searches = roster.missing.blocking_lock().searches();
        for _ in 0..20 {
            roster
                .activity(codex())
                .expect("unchanged missing location");
        }
        assert_eq!(roster.missing.blocking_lock().searches(), searches);

        fs::write(&log, b"").expect("provider publishes a log before its opening record");
        roster.activity(codex()).expect("created empty log");
        for _ in 0..20 {
            roster.activity(codex()).expect("unchanged empty header");
        }
        assert_eq!(
            roster
                .followed
                .blocking_lock()
                .get(OPEN_THREAD)
                .expect("followed log")
                .head_reads,
            1,
            "an absent field at an unchanged stamp must not reread the header"
        );
        fs::write(
            &log,
            b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"C:/late-folder\"}}\n",
        )
        .expect("provider publishes the original folder");
        let changed = roster.activity(codex()).expect("changed header");
        assert_eq!(
            changed
                .processes
                .first()
                .expect("exact owner")
                .cwd
                .as_deref(),
            Some("C:/late-folder")
        );
        assert_eq!(
            roster
                .followed
                .blocking_lock()
                .get(OPEN_THREAD)
                .expect("followed log")
                .head_reads,
            2
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_retained_holder_accepts_its_later_folder_without_another_process_lookup() {
        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let _held = hold(&root, OPEN_THREAD);
        let mut roster = CodexRoster::rooted(root.clone());
        let first = roster
            .observation(codex())
            .expect("the writer precedes its metadata");
        assert_eq!(
            first
                .activity
                .processes
                .first()
                .expect("retained holder")
                .cwd,
            None
        );
        roster.ask_holder = |_| panic!("the exact live owner was already retained");
        let log = locate_log(&root, OPEN_THREAD).expect("the empty source exists");
        fs::write(
            log,
            b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"C:/fixture\"}}\n",
        )
        .expect("the provider publishes its original folder");
        let second = roster
            .observation(codex())
            .expect("the new source metadata is visible");
        assert_eq!(
            second
                .activity
                .processes
                .first()
                .expect("retained holder")
                .cwd
                .as_deref(),
            Some("C:/fixture")
        );
        assert_ne!(first.catalogue_revision, second.catalogue_revision);
        assert!(second.activity.active.is_empty());
        assert_eq!(second.unknown_activity.len(), 1);
    }
}
