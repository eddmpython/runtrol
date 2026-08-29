//! Whether an editor-panel Claude session has a turn open, read from its transcript's turn boundaries.
//!
//! A session the Claude editor extension drives writes no `status` into the process roster (measured
//! 2026-08-30: a `claude-vscode` record carries `pid`, `cwd`, `sessionId` and names, and no status at all),
//! so the roster alone cannot say whether its model is answering. The transcript is a structured surface all
//! the same: every assistant message records a `stop_reason`, and the last one in the file says whether the
//! turn is still open. This reads only those byte markers and the record `type`, never the message text, and
//! keeps no copy. It is the same nameplate-only rule the catalogue reads a stored conversation's title by
//! (`thinPrinciple.md`): structural keys found as bytes, no body decoded.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{fs, io};

use tokio::sync::Mutex;

const PROJECTS_DIRECTORY: &str = "projects";
const TRANSCRIPT_EXTENSION: &str = "jsonl";

/// The turn is open while the last assistant message paused for a tool.
const TURN_TOOL: &[u8] = b"\"stop_reason\":\"tool_use\"";
/// The turn is closed when the last assistant message ended for the person, either way it can end.
const TURN_END: &[u8] = b"\"stop_reason\":\"end_turn\"";
const TURN_STOP: &[u8] = b"\"stop_reason\":\"stop_sequence\"";
/// A person's prompt records as a user message. After the last closed turn it is a turn starting before its
/// first assistant token is written; a tool result records the same way but only ever before a turn closes.
const USER_RECORD: &[u8] = b"\"type\":\"user\"";

/// The tail read from the end when the file grew. A turn marker is written on every assistant message, so the
/// final quarter-megabyte almost always holds one; a window with none is doubled up to the ceiling.
const FIRST_WINDOW_BYTES: u64 = 256 * 1024;
/// The most this reads backwards before answering "cannot tell", which is reported as no turn open.
const MAX_WINDOW_BYTES: u64 = 8 * 1024 * 1024;
/// The most project directories one search for a transcript walks. One per folder the CLI ever ran in.
const MAX_PROJECT_DIRECTORIES: usize = 8192;

/// An open turn whose transcript has not grown in this long is read as ended. A turn stopped at the keyboard
/// leaves the last marker `tool_use` with nothing after it, and only the clock tells that from a live pause.
const OPEN_TURN_FRESH: Duration = Duration::from_secs(45);
/// How long a "no transcript for this session yet" answer is kept before the directories are searched again,
/// so a session in the first moment before its file exists is not searched for on every observation.
const MISS_RETRY: Duration = Duration::from_secs(5);

/// What one session's transcript looked like at the last observation.
#[derive(Clone, Debug)]
struct Followed {
    /// The transcript this session writes to.
    transcript: PathBuf,
    /// Its length then, so an unchanged file is answered from `answering` without another read.
    size: u64,
    /// Whether a turn was open at that point.
    answering: bool,
}

/// A session whose transcript could not be located, and when that was last tried.
#[derive(Clone, Debug)]
struct Missing {
    at: SystemTime,
}

#[derive(Clone, Debug)]
enum Known {
    Found(Followed),
    NotYet(Missing),
}

/// Reads whether a session's model is answering from its transcript, keeping each file's last size and state
/// so an unchanged transcript costs one `stat` and a growing one is read only over what it grew by.
#[derive(Clone, Debug)]
pub(super) struct TranscriptActivity {
    /// `<config>/projects`, or `None` when the CLI keeps no home this process can find.
    projects: Option<PathBuf>,
    /// Per session, shared because the driver hands clones of the roster to the blocking pool.
    seen: Arc<Mutex<HashMap<Box<str>, Known>>>,
    /// The clock, so tests can pin freshness. The product reads the wall clock.
    now: fn() -> SystemTime,
}

impl TranscriptActivity {
    pub(super) fn new(projects: Option<PathBuf>) -> Self {
        Self {
            projects: projects.map(|config| config.join(PROJECTS_DIRECTORY)),
            seen: Arc::new(Mutex::new(HashMap::new())),
            now: SystemTime::now,
        }
    }

    #[cfg(test)]
    fn rooted(projects: PathBuf, now: fn() -> SystemTime) -> Self {
        Self {
            projects: Some(projects),
            seen: Arc::new(Mutex::new(HashMap::new())),
            now,
        }
    }

    /// Whether this session has a turn open, reading only what its transcript grew by since the last look.
    pub(super) fn answering(&self, session: &str) -> bool {
        let Some(projects) = &self.projects else {
            return false;
        };
        let mut seen = self.seen.blocking_lock();
        let cached = match seen.get(session) {
            Some(Known::Found(followed)) => Some(followed.transcript.clone()),
            Some(Known::NotYet(missing)) if elapsed(self.now, missing.at) < MISS_RETRY => {
                return false;
            }
            _ => None,
        };
        let transcript = if let Some(path) = cached {
            path
        } else if let Some(path) = locate_transcript(projects, session) {
            path
        } else {
            seen.insert(session.into(), Known::NotYet(Missing { at: (self.now)() }));
            return false;
        };
        let Ok(metadata) = fs::metadata(&transcript) else {
            // The transcript went away underneath a live process: nothing can be said about a turn, and the
            // path is dropped so a session that reappears is located again.
            seen.remove(session);
            return false;
        };
        let size = metadata.len();
        if let Some(Known::Found(followed)) = seen.get(session)
            && followed.size == size
        {
            return followed.answering && fresh(self.now, &metadata);
        }
        let answering = last_turn_open(&transcript, size).unwrap_or(false);
        seen.insert(
            session.into(),
            Known::Found(Followed {
                transcript,
                size,
                answering,
            }),
        );
        answering && fresh(self.now, &metadata)
    }
}

/// How long since `at`, saturating at zero so a clock that stepped back is not a negative age.
fn elapsed(now: fn() -> SystemTime, at: SystemTime) -> Duration {
    now().duration_since(at).unwrap_or(Duration::ZERO)
}

/// Whether the transcript was written to recently enough that an open turn is a live pause, not an abandoned one.
fn fresh(now: fn() -> SystemTime, metadata: &fs::Metadata) -> bool {
    let Ok(modified) = metadata.modified() else {
        // A file system that does not report a modification time cannot disqualify an open turn, so the
        // structural answer stands on its own.
        return true;
    };
    now().duration_since(modified).unwrap_or(Duration::ZERO) < OPEN_TURN_FRESH
}

/// The transcript file this session writes to, `projects/<any folder slug>/<session>.jsonl`.
///
/// The folder slug the CLI builds from the working directory is lossy (its drive letter is upper-cased and
/// every other non-alphanumeric byte becomes a dash), so the folder is not computed from the workspace; the
/// directories are few, one per folder the CLI ever ran in, and asking each is cheaper than guessing wrong.
/// The same reasoning the store locates a conversation by (`store.rs`).
fn locate_transcript(projects: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return None;
    }
    let file_name = format!("{session}.{TRANSCRIPT_EXTENSION}");
    // A projects directory that cannot be listed holds no transcript this can name, which is the same answer
    // a missing one gives: the session is reported as not answering rather than guessed about.
    let Ok(directories) = fs::read_dir(projects) else {
        return None;
    };
    directories
        .flatten()
        .take(MAX_PROJECT_DIRECTORIES)
        .map(|entry| entry.path().join(&file_name))
        .find(|candidate| candidate.is_file())
}

/// Whether the transcript's last turn marker leaves a turn open, reading its tail backwards.
///
/// Widens the tail until it holds a turn marker, up to [`MAX_WINDOW_BYTES`]; a file whose last marker is
/// further back than that is reported as "cannot tell" (`None`), which the caller reads as no turn open.
fn last_turn_open(path: &Path, size: u64) -> Option<bool> {
    let mut window = FIRST_WINDOW_BYTES;
    loop {
        let start = size.saturating_sub(window);
        // A transcript that cannot be read says nothing about a turn: the caller reads None as not answering.
        let Ok(tail) = read_range(path, start, size) else {
            return None;
        };
        // A window that did not begin at the file's start opens inside a record; drop that fragment so a
        // half-read marker at the edge cannot be matched or missed.
        let bytes = if start > 0 {
            match memchr(tail.as_slice(), b'\n') {
                Some(newline) => tail.get(newline + 1..).unwrap_or_default(),
                None => tail.as_slice(),
            }
        } else {
            tail.as_slice()
        };
        if let Some(open) = turn_state(bytes) {
            return Some(open);
        }
        if start == 0 || window >= MAX_WINDOW_BYTES {
            return None;
        }
        window = (window * 2).min(MAX_WINDOW_BYTES);
    }
}

/// Whether the last turn marker in these bytes leaves a turn open, or `None` when they hold no marker.
///
/// The last marker decides: a `tool_use` is a turn still going, an `end_turn` or `stop_sequence` is a turn
/// finished, unless a user record follows it, which is the next turn beginning before its assistant reply is
/// written (a plain text answer records no `stop_reason` of its own until it ends, so the open turn is seen
/// only by the person's prompt sitting after the last closed one).
fn turn_state(bytes: &[u8]) -> Option<bool> {
    let tool = rfind(bytes, TURN_TOOL);
    let closed = rfind(bytes, TURN_END).max(rfind(bytes, TURN_STOP));
    match (tool, closed) {
        (None, None) => None,
        (Some(_), None) => Some(true),
        (None, Some(close)) => Some(user_after(bytes, close)),
        (Some(open), Some(close)) => {
            if open > close {
                Some(true)
            } else {
                Some(user_after(bytes, close))
            }
        }
    }
}

/// Whether a user record begins after byte `after` in these bytes.
fn user_after(bytes: &[u8], after: usize) -> bool {
    rfind(bytes, USER_RECORD).is_some_and(|user| user > after)
}

/// Read bytes `[start, end)` of a file into a buffer.
fn read_range(path: &Path, start: u64, end: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let length = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
    let mut bytes = vec![0_u8; length];
    let mut filled = 0;
    while let Some(slot) = bytes.get_mut(filled..) {
        if slot.is_empty() {
            break;
        }
        let read = file.read(slot)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    bytes.truncate(filled);
    Ok(bytes)
}

/// The last start offset of `needle` in `haystack`, or `None`.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// The first offset of `byte` in `haystack`, or `None`.
fn memchr(haystack: &[u8], byte: u8) -> Option<usize> {
    haystack.iter().position(|&candidate| candidate == byte)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "runtrol-claude-activity-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("the scratch root is created");
            Self { root }
        }

        /// Write a transcript for `session` under a folder slug, returning the projects directory.
        fn transcript(&self, slug: &str, session: &str, body: &str) -> PathBuf {
            let projects = self.root.join(PROJECTS_DIRECTORY);
            let folder = projects.join(slug);
            fs::create_dir_all(&folder).expect("the folder is created");
            let mut file = File::create(folder.join(format!("{session}.jsonl")))
                .expect("the transcript opens");
            file.write_all(body.as_bytes())
                .expect("the transcript is written");
            projects
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.root));
        }
    }

    const RECENT: fn() -> SystemTime = SystemTime::now;

    /// A clock forty-six seconds ahead of a freshly written file, so an open marker in it reads as stale.
    fn stale() -> SystemTime {
        SystemTime::now() + Duration::from_secs(46)
    }

    fn assistant(stop: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"stop_reason\":\"{stop}\"}}}}\n"
        )
    }

    fn user() -> String {
        "{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n".to_owned()
    }

    #[test]
    fn a_turn_paused_for_a_tool_is_open() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000001";
        let body = format!(
            "{}{}{}",
            assistant("end_turn"),
            user(),
            assistant("tool_use")
        );
        let projects = scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert_eq!(activity.projects, Some(projects));
        assert!(activity.answering(session));
    }

    #[test]
    fn a_turn_that_ended_is_closed() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000002";
        let body = format!(
            "{}{}{}",
            user(),
            assistant("tool_use"),
            assistant("end_turn")
        );
        scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(!activity.answering(session));
    }

    #[test]
    fn a_prompt_after_the_last_closed_turn_is_a_turn_starting() {
        // A plain text reply writes no stop_reason of its own until it ends: the open turn shows only as the
        // person's prompt sitting after the last closed turn.
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000003";
        let body = format!("{}{}", assistant("end_turn"), user());
        scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(activity.answering(session));
    }

    #[test]
    fn a_tool_result_before_the_close_does_not_reopen_a_finished_turn() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000004";
        // user here is a tool result, and it sits before the closing end_turn, so the turn is finished.
        let body = format!(
            "{}{}{}",
            assistant("tool_use"),
            user(),
            assistant("end_turn")
        );
        scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(!activity.answering(session));
    }

    #[test]
    fn an_open_turn_whose_transcript_went_quiet_is_read_as_ended() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000005";
        scratch.transcript("slug", session, &assistant("tool_use"));
        // The clock runs ahead of the file's fresh modification time, so the open marker is stale.
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), stale);
        assert!(!activity.answering(session));
    }

    #[test]
    fn a_session_with_no_transcript_is_not_answering() {
        let scratch = Scratch::new();
        fs::create_dir_all(scratch.root.join(PROJECTS_DIRECTORY)).expect("projects exists");
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(!activity.answering("bbbbbbbb-0000-4000-8000-000000000001"));
    }

    #[test]
    fn a_grown_transcript_is_re_read_and_an_unchanged_one_is_not() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000006";
        let projects = scratch.transcript("slug", session, &assistant("tool_use"));
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(activity.answering(session));
        // The turn ends: the file grows, and the next look reads the appended close.
        let path = projects.join("slug").join(format!("{session}.jsonl"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the transcript reopens");
        file.write_all(assistant("end_turn").as_bytes())
            .expect("the close is appended");
        drop(file);
        assert!(!activity.answering(session));
    }

    #[test]
    fn the_last_marker_across_a_window_edge_is_found_by_widening() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000007";
        // A long run of user records after the close, longer than the first window, still resolves to open
        // because the search widens until it holds the closing marker and the prompt that follows it.
        let filler = user().repeat(20_000);
        let body = format!(
            "{}{}{}",
            assistant("tool_use"),
            assistant("end_turn"),
            filler
        );
        scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(activity.answering(session));
    }
}
