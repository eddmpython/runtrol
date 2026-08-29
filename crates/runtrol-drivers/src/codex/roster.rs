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
//! `task_started` rather than `task_complete`. Both finished conversations examined ended on `task_complete`;
//! the one that was answering had `task_complete` at 07:20:36.672Z followed by `task_started` at
//! 07:20:36.804Z, and everything written since. This is a state, not a heartbeat, so it stays true through a
//! long tool call that writes nothing for minutes. That is what a timestamp cannot do, and why the timestamp
//! is not the signal (`memory/sidebarContract.md` records the same mistake made for the other CLI).
//!
//! Nothing here reads a message. The lock is a file name, and the log is read for the names of its event
//! types and the byte offset reached.
//!
//! # What it costs
//!
//! One directory listing and one small open per lock, then, for each conversation a live process owns, the
//! bytes its log grew by since the last look. A conversation nobody touched costs one `metadata` call. The
//! first sight of a conversation scans backwards from the end for its last boundary, bounded, because a log
//! that has been running for days is measured in hundreds of megabytes.

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use runtrol_provider::{NativeProcessActivity, NativeSessionId, ProviderError, ProviderId};

use crate::operator::{HomeProblem, provider_home};

/// The CLI's own override for where it keeps everything.
const HOME_ENV: &str = "CODEX_HOME";

/// The default folder under the operator's home.
const HOME_DEFAULT: &str = ".codex";

/// One lock file per conversation a process has open, named for the conversation.
const LOCKS_DIRECTORY: &str = "thread-writer-locks";

/// Where the per-conversation event logs live, under `<year>/<month>/<day>`.
const SESSIONS_DIRECTORY: &str = "sessions";

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
const TURN_OPENED: &[u8] = b"\"type\":\"task_started\"";

/// The event that closes one.
const TURN_CLOSED: &[u8] = b"\"type\":\"task_complete\"";

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
    /// Whether a turn was open at that point.
    answering: bool,
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
    /// The last answer about which conversations a live process holds, reused for [`Self::owned_for`].
    owned: Arc<Mutex<Option<Ownership>>>,
    /// How long that answer is reused. [`OWNERSHIP_FOR`] in the product; nothing in tests, which assert on
    /// what one call sees rather than on a clock.
    owned_for: Duration,
    /// Where each conversation's log is and how far it has been read, so a later look reads only what was
    /// appended. Shared because the driver hands clones to the blocking pool.
    followed: Arc<Mutex<HashMap<Box<str>, Followed>>>,
}

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

    fn holding(home: Result<PathBuf, HomeProblem>, owned_for: Duration) -> Self {
        Self {
            home,
            owned: Arc::new(Mutex::new(None)),
            owned_for,
            followed: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    fn rooted(home: PathBuf) -> Self {
        Self::holding(Ok(home), Duration::ZERO)
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
        // Conversations nobody owns any more stop being followed, so a machine that has run for a week is
        // not carrying an entry per conversation it once saw.
        // Taken with the blocking form on purpose: every caller reaches this from the blocking pool (the
        // driver's `native_process_activity` spawns it there), and the async form cannot be awaited from
        // outside a runtime task.
        let mut followed = self.followed.blocking_lock();
        followed.retain(|thread, _| owned.iter().any(|owned| owned.as_str() == thread.as_ref()));
        for thread in owned {
            let answering = answering(home, &mut followed, thread.as_str());
            if answering {
                active.push(thread.clone());
            }
            live.push(thread);
        }
        Ok(NativeProcessActivity {
            live,
            active,
            // This CLI's locks name the conversation, never the process that holds it. A terminal runtrol
            // hosts is bound by the identity its own child reported, and nothing here can bind one it did
            // not start, so no binding is claimed.
            processes: Vec::new(),
        })
    }
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
        if path.extension().and_then(|extension| extension.to_str()) != Some(LOCK_EXTENSION) {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(thread) = NativeSessionId::new(name) else {
            // The coordination lock beside them is not a conversation, and neither is anything else the
            // CLI puts here that its own identity rules would refuse.
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
fn answering(home: &Path, followed: &mut HashMap<Box<str>, Followed>, thread: &str) -> bool {
    if let Some(known) = followed.get_mut(thread) {
        let Ok(size) = fs::metadata(&known.log).map(|metadata| metadata.len()) else {
            // The log went away underneath a live process: nothing can be said about a turn, and saying
            // "answering" about a conversation with no log is the one answer that would be wrong.
            return false;
        };
        if size == known.read_to {
            return known.answering;
        }
        if size < known.read_to || size - known.read_to > MAX_FOLLOW_BYTES {
            // Truncated, replaced, or grown further than one look may read. Ask the file again from the
            // end rather than trusting an offset into a file that is no longer the same one.
            let Some(fresh) = last_boundary(&known.log) else {
                return known.answering;
            };
            known.read_to = fresh.0;
            known.answering = fresh.1;
            return known.answering;
        }
        if let Some(state) = boundary_in_range(&known.log, known.read_to, size) {
            known.answering = state;
        }
        known.read_to = size;
        return known.answering;
    }
    let Some(log) = locate_log(home, thread) else {
        return false;
    };
    let Some((read_to, answering)) = last_boundary(&log) else {
        return false;
    };
    followed.insert(
        thread.into(),
        Followed {
            log,
            read_to,
            answering,
        },
    );
    answering
}

/// The log file this conversation writes to.
///
/// The CLI names it `rollout-<timestamp>-<thread>.jsonl` under a day directory, so the search is for a
/// suffix rather than a guess at the timestamp. Bounded, and done once per conversation: the answer is kept
/// for as long as a live process owns it.
fn locate_log(home: &Path, thread: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread}.{LOG_EXTENSION}");
    let sessions = home.join(SESSIONS_DIRECTORY);
    let mut days = Vec::new();
    collect_day_directories(&sessions, 0, &mut days);
    // Newest first: a conversation a process is holding was written to recently, and the day directories
    // sort by name into calendar order.
    days.sort_by(|left, right| right.cmp(left));
    for day in days.into_iter().take(MAX_DAY_DIRECTORIES) {
        let Ok(entries) = fs::read_dir(&day) else {
            continue;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DAY_ENTRIES {
                break;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
            {
                return Some(path);
            }
        }
    }
    None
}

/// Every `<year>/<month>/<day>` directory under the sessions root, three levels down.
fn collect_day_directories(at: &Path, depth: usize, into: &mut Vec<PathBuf>) {
    if depth == 3 {
        into.push(at.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(at) else {
        return;
    };
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DAY_ENTRIES || into.len() >= MAX_DAY_DIRECTORIES {
            return;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            collect_day_directories(&path, depth + 1, into);
        }
    }
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
fn last_boundary(log: &Path) -> Option<(u64, bool)> {
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
    let straddle = u64::try_from(TURN_OPENED.len().max(TURN_CLOSED.len()) - 1).unwrap_or(0);
    let mut end = size;
    while end > floor {
        let start = end.saturating_sub(SCAN_CHUNK_BYTES).max(floor);
        let stop = (end + straddle).min(size);
        let bytes = read_range(&mut file, start, stop)?;
        if let Some(open) = last_open_turn(&bytes) {
            return Some((size, open));
        }
        end = start;
    }
    Some((size, false))
}

/// The state of the last boundary inside one range of the log, or `None` when the range holds none.
fn boundary_in_range(log: &Path, from: u64, to: u64) -> Option<bool> {
    let Ok(mut file) = fs::File::open(log) else {
        return None;
    };
    let bytes = read_range(&mut file, from, to)?;
    last_open_turn(&bytes)
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
fn last_open_turn(bytes: &[u8]) -> Option<bool> {
    let opened = last_index_of(bytes, TURN_OPENED);
    let closed = last_index_of(bytes, TURN_CLOSED);
    match (opened, closed) {
        (None, None) => None,
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (Some(opened), Some(closed)) => Some(opened > closed),
    }
}

fn last_index_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .rfind(|(_, window)| *window == needle)
        .map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn codex() -> ProviderId {
        ProviderId::parse("codex").expect("the built-in provider identity parses")
    }

    const OPEN_THREAD: &str = "01a03afd-5184-7512-8b48-b77dd957d18a";
    const DONE_THREAD: &str = "01a0471d-786c-7561-a8d5-db5ddb837c0c";

    fn opened(id: &str) -> String {
        format!(
            "{{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"{id}\"}}}}\n"
        )
    }

    fn closed(id: &str) -> String {
        format!(
            "{{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"turn_id\":\"{id}\"}}}}\n"
        )
    }

    fn chatter() -> String {
        "{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{}}}\n"
            .to_owned()
    }

    /// A home shaped the way the CLI shapes its own, with the given conversations and logs.
    fn home(logs: &[(&str, String)]) -> (Scratch, PathBuf) {
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
        assert!(activity.processes.is_empty());
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
            println!("  {} answering={answering}", thread.as_str());
        }
    }

    #[test]
    fn the_last_boundary_in_a_window_decides_and_no_boundary_decides_nothing() {
        assert_eq!(last_open_turn(chatter().as_bytes()), None);
        assert_eq!(last_open_turn(opened("a").as_bytes()), Some(true));
        assert_eq!(last_open_turn(closed("a").as_bytes()), Some(false));
        assert_eq!(
            last_open_turn((closed("a") + &opened("b")).as_bytes()),
            Some(true)
        );
        assert_eq!(
            last_open_turn((opened("a") + &chatter() + &closed("a")).as_bytes()),
            Some(false)
        );
    }
}
