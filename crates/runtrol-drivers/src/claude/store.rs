//! The conversations this CLI has stored, named from its own store.
//!
//! # Why the store, and what is read from it
//!
//! This CLI publishes no command or protocol method that lists what it has stored (measured 2.1.238: `claude
//! agents --json` is a roster of running processes, `claude project` only purges, and `--resume` without an
//! identifier opens a terminal picker). The conversations themselves live where the CLI resumes them from:
//! `<config>/projects/<folder slug>/<session id>.jsonl`, one file per conversation. So that is where their
//! names come from.
//!
//! What is read is exactly what a catalogue row is made of, and nothing a conversation is made of:
//!
//! - the **identity**, which is the file name (the CLI resumes by it);
//! - the **folder**, which is the `cwd` field the CLI writes on every message record (the directory slug is
//!   lossy: `_`, `.`, `:` and the separators all become `-`, so it cannot be inverted);
//! - the **title**, which is the CLI's own `aiTitle` record when it has written one, otherwise the first
//!   structured `display` preview its prompt-history surface associates with the same session identity;
//! - the **time**, which is the file's modification time, the CLI's own last write.
//!
//! No transcript message is decoded to invent a title. The folder and explicit title are found as bare JSON keys
//! by a rolling scan over the conversation file. A key spelled with unescaped quotes can only occur at a
//! structural position, never inside a string, so a message quoting `"cwd"` cannot be mistaken for the record's
//! own. Measured on 266 stored conversations:
//! the first `cwd` sat within 220 KiB of the start in 264 of them, one sat at 1.08 MB behind a pasted first
//! message, and one file (a renamed, never-used conversation) has no message and so no folder at all. The scan
//! holds one window in memory however long the file is, stops as soon as the folder and the title are known,
//! and a conversation that names no folder is counted and said, never guessed.
//!
//! # What this is not
//!
//! Not a transcript copy and not a search. Conversation files are opened read-only, their heads are scanned for
//! two structural keys, and nothing is retained. The separate prompt-history file is also provider-owned and is
//! streamed only for its bounded `sessionId` and `display` fields. The thin principle names this store as the
//! conversation's one home; this module only learns the names on its doors.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use runtrol_provider::{
    MAX_NATIVE_SESSION_ITEMS, NativeCatalogueCoverage, NativeCatalogueSource,
    NativeResumeCapability, NativeSessionCatalogue, NativeSessionEntry, NativeSessionId,
    NativeSessionQuery, ProviderError, ProviderId,
};

use crate::catalogue::{bounded, under};
use crate::claude::home::{HomeProblem, config_directory};
use crate::claude::trash;

/// Where the CLI keeps one directory per folder it has run in.
const PROJECTS_DIRECTORY: &str = "projects";

/// The CLI's structured prompt-history list. Its `display` field is the same human-facing preview the CLI owns;
/// it is used only when a conversation has no explicit `aiTitle` record.
const HISTORY_FILE: &str = "history.jsonl";

/// The extension of a stored conversation. Everything else in a project directory is the CLI's own business.
const CONVERSATION_EXTENSION: &str = "jsonl";

/// How much is read at a time. Most files answer within the first step.
const STEP: usize = 64 * 1024;

/// How far a file is scanned for its folder at most. A conversation is placed by its first message's record,
/// and a first message past this is not one this surface can place; said in the coverage, never guessed.
const FOLDER_BUDGET: u64 = 64 * 1024 * 1024;

/// How far the title is looked for. Measured: the latest title sat at 1.1 MB and most within the first
/// exchange; a conversation without one simply has no title, and a bound here is what keeps the untitled ones
/// from being read to their end on every listing.
const TITLE_BUDGET: u64 = 2 * 1024 * 1024;

/// The longest string a key's value may run across a window boundary. A folder or a title longer than this is
/// not one, and the scan stops carrying it.
const MAX_VALUE_BYTES: usize = 8 * 1024;

/// How many stored conversations are indexed at most. Far above any store measured, and a ceiling on the
/// memory one listing may hold rather than a quota anyone is expected to meet.
const MAX_INDEXED: usize = 10_000;

/// Prompt history can contain several entries for one conversation. This ceiling is well beyond the measured
/// store while keeping a corrupt or hostile provider file from building an unbounded catalogue map.
const MAX_HISTORY_ROWS: usize = 100_000;

/// The record field that names the folder a conversation ran in, as the CLI spells it.
const FOLDER_KEY: &[u8] = b"\"cwd\":\"";

/// The record field carrying the CLI's own title for the conversation, as the CLI spells it.
const TITLE_KEY: &[u8] = b"\"aiTitle\":\"";

/// How far back from the end of a conversation file a resume reads. A bound on bytes, not on records, because
/// a record can be a pasted megabyte; what remains is split into whole lines and the first partial line dropped.
const REPLAY_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// How many message records a resume replays at most, the same bound the Codex driver applies to the items
/// its `thread/resume` hands over, so a reopened conversation reads the same length on either service.
const MAX_REPLAY_RECORDS: usize = 64;

/// How many bytes of records a resume replays at most. The Runtime keeps 64 KiB of a session's events for
/// a subscriber that joins late (the memory contract), and a tab subscribes moments after the resume; records
/// past this would be relayed and forgotten before anybody saw them, so the tail is cut here instead.
const MAX_REPLAY_BYTES: usize = 48 * 1024;

/// The tail of one stored conversation, as the CLI wrote it, for a resume to show.
///
/// The CLI's own `--resume` in a terminal draws the stored conversation back; its stream-json mode prints none
/// of it. So the driver reads what the CLI would have drawn, from the same file, and hands each record to the
/// same frame reader a live frame goes through (a stored `assistant` or `user` record is the same shape as the
/// live frame). Nothing is interpreted here: only which lines are message records is decided, from their
/// `type`, and the rest is relayed as it is.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Replay {
    /// Message records, oldest first, bounded.
    pub(crate) records: Vec<Bytes>,
    /// Why the tail could not be read, when it could not; the agent says so once and goes on.
    pub(crate) problem: Option<Box<str>>,
}

/// The store, located once from the environment the CLI itself reads.
#[derive(Clone, Debug)]
pub(super) struct ClaudeStore {
    projects: Result<PathBuf, HomeProblem>,
}

impl ClaudeStore {
    /// Locate the store from the environment inherited by the CLI. Opens nothing.
    #[must_use]
    pub(super) fn from_environment() -> Self {
        Self {
            projects: config_directory(&mut |name| std::env::var_os(name))
                .map(|directory| directory.join(PROJECTS_DIRECTORY)),
        }
    }

    #[cfg(test)]
    fn at(projects: PathBuf) -> Self {
        Self {
            projects: Ok(projects),
        }
    }

    /// One page of stored conversations, newest first.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the store exists and cannot be read, or the cursor is not one this
    /// store issued. A store that does not exist yet is an empty, complete answer: a fresh install has stored
    /// nothing, and saying so is true.
    pub(super) fn list(
        &self,
        provider: ProviderId,
        resumable: bool,
        query: &NativeSessionQuery,
    ) -> Result<NativeSessionCatalogue, ProviderError> {
        let projects = match &self.projects {
            Ok(projects) => projects,
            Err(problem) => {
                return Ok(NativeSessionCatalogue::unsupported(format!(
                    "the store of this CLI could not be located: {problem}"
                )));
            }
        };
        let indexed = index(provider, projects)?;
        let display_titles = projects
            .parent()
            .map(|config| history_titles(&config.join(HISTORY_FILE)))
            .unwrap_or_default();
        let after = query
            .cursor
            .as_deref()
            .map(|cursor| Position::decode(provider, cursor))
            .transpose()?;
        Ok(page(
            &indexed,
            &display_titles,
            after.as_ref(),
            query,
            resumable,
        ))
    }
}

impl ClaudeStore {
    /// The conversations this CLI wrote in the last `within`.
    ///
    /// Names and times only. The listing above opens every file to read the folder it ran in and the title the
    /// CLI gave it, which costs 121 ms on the operator's machine; this asks the filesystem for the directory
    /// entries and their write times and costs 23 ms. A conversation whose transcript is being written is one
    /// whose model is answering, and that is the whole question here.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the store exists and cannot be walked. A store that is not there yet
    /// is an empty answer, the same as a store where nothing has been written lately.
    pub(super) fn active(
        &self,
        provider: ProviderId,
        within: Duration,
    ) -> Result<Vec<NativeSessionId>, ProviderError> {
        let Ok(projects) = &self.projects else {
            return Ok(Vec::new());
        };
        let read_failure = |detail: std::io::Error| ProviderError::Protocol {
            provider,
            doing: "reading which conversations this CLI wrote lately",
            detail: detail.to_string(),
        };
        let directories = match fs::read_dir(projects) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(read_failure(error)),
        };
        let now = SystemTime::now();
        let mut active = Vec::new();
        for project in directories {
            let project = project.map_err(read_failure)?;
            if !project.file_type().map_err(read_failure)?.is_dir() {
                continue;
            }
            let files = match fs::read_dir(project.path()) {
                Ok(files) => files,
                // A project folder that disappeared between the two reads is not an error about the store.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(read_failure(error)),
            };
            for file in files {
                let file = file.map_err(read_failure)?;
                let path = file.path();
                if path.extension().and_then(|extension| extension.to_str())
                    != Some(CONVERSATION_EXTENSION)
                {
                    continue;
                }
                let Ok(metadata) = file.metadata() else {
                    continue;
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                let Ok(age) = now.duration_since(modified) else {
                    // A write stamped in the future is a clock that moved, not a conversation from the future.
                    continue;
                };
                if age > within {
                    continue;
                }
                let Some(Ok(native)) = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(NativeSessionId::new)
                else {
                    continue;
                };
                active.push(native);
            }
        }
        Ok(active)
    }

    /// The most recent message records of one stored conversation, for a resume to replay.
    ///
    /// A conversation the store does not hold (or a store that cannot be located) replays nothing, which is
    /// what a fresh or foreign identifier should do; a file that exists and cannot be read says so.
    #[must_use]
    pub(super) fn recent_records(&self, native: &str) -> Replay {
        let Ok(projects) = &self.projects else {
            return Replay::default();
        };
        let Some(path) = conversation_path(projects, native) else {
            return Replay::default();
        };
        match tail_records(&path) {
            Ok(records) => Replay {
                records,
                problem: None,
            },
            Err(error) => Replay {
                records: Vec::new(),
                problem: Some(
                    format!("the stored conversation could not be read back: {error}").into(),
                ),
            },
        }
    }

    /// Move one stored conversation out of the store, reversibly.
    ///
    /// runtrol reads this store to name conversations (the driver contract on the catalogue); deleting is that
    /// same contract carried to its lifecycle end. The conversation file and its side-transcript directory are
    /// moved into `runtrol-deleted`, a sibling of `projects` the listing never walks, rather than erased, so the
    /// act is reversible by hand and the provider's store stays the record of what exists. A name the store no
    /// longer holds is already in the asked-for state and answers as done. Reachable only under the delete scope
    /// the Runtime grants from the machine.
    ///
    /// # Errors
    ///
    /// The underlying filesystem error when the move cannot be made: a file the CLI still holds open, a store
    /// that could not be located, or a trash that could not be created.
    pub(super) fn delete(&self, native: &str) -> std::io::Result<()> {
        let projects = self.projects.as_ref().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the stored conversation directory could not be located",
            )
        })?;
        let Some(file) = conversation_path(projects, native) else {
            return Ok(());
        };
        // The move itself lives in `trash`, which is the driver's only writer of this store.
        trash::discard(projects, &file, native)
    }
}

/// Where the store keeps one conversation: `projects/<any folder slug>/<native>.jsonl`.
///
/// The slug is lossy, so the folder is not computed from the workspace; the directories are few (one per
/// folder the CLI ever ran in) and asking each is cheaper than guessing wrong.
fn conversation_path(projects: &Path, native: &str) -> Option<PathBuf> {
    if native.is_empty() || native.contains(['/', '\\', '.']) {
        return None;
    }
    let file_name = format!("{native}.{CONVERSATION_EXTENSION}");
    // A store that cannot be listed holds no conversation this driver can name, which is the same answer a
    // missing store gives; the listing path says why when it matters.
    let Ok(directories) = fs::read_dir(projects) else {
        return None;
    };
    directories
        .flatten()
        .map(|entry| entry.path().join(&file_name))
        .find(|candidate| candidate.is_file())
}

/// Read the bounded tail of the file and keep the message records in it, oldest first.
fn tail_records(path: &Path) -> std::io::Result<Vec<Bytes>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(REPLAY_TAIL_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length - start).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    let mut lines = bytes.split(|byte| *byte == b'\n');
    if start > 0 {
        // The first line of a tail that did not begin at the file's start is a fragment of a record.
        lines.next();
    }
    let mut records: Vec<Bytes> = lines
        .filter(|line| is_message_record(line))
        .map(Bytes::copy_from_slice)
        .collect();
    let skip = records.len().saturating_sub(MAX_REPLAY_RECORDS);
    records.drain(..skip);
    // Newest first is what survives a byte cut: the records are oldest-first, so the oldest go.
    let mut bytes: usize = records.iter().map(Bytes::len).sum();
    while bytes > MAX_REPLAY_BYTES && records.len() > 1 {
        let oldest = records.remove(0);
        bytes = bytes.saturating_sub(oldest.len());
    }
    // A tail that opens mid-exchange (a tool result whose call was cut, a reply whose question was cut)
    // reads as nothing on the page; the page draws a tool result under the call it belongs to. So the tail
    // opens at the newest prompt that still fits, when there is one, and the exchange reads whole.
    if let Some(first_prompt) = records.iter().position(|record| is_prompt_record(record)) {
        records.drain(..first_prompt);
    }
    Ok(records)
}

/// Whether a message record is the operator's own prompt (a `user` record that is not a tool result). The
/// CLI marks a tool result's record with its `toolUseResult`, so the absence of that key on a `user` record
/// is what a prompt looks like.
fn is_prompt_record(line: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Head<'line> {
        #[serde(rename = "type")]
        kind: Option<&'line str>,
        #[serde(rename = "toolUseResult", default, borrow)]
        tool_use_result: Option<&'line serde_json::value::RawValue>,
    }
    let Ok(head) = serde_json::from_slice::<Head<'_>>(line) else {
        return false;
    };
    head.kind == Some("user") && head.tool_use_result.is_none()
}

/// Whether a stored line is a message record: the CLI's `user` or `assistant` type with a `message`, on the
/// conversation's own thread. Everything else in the file (titles, queue marks, snapshots, sub-agent
/// sidechains) is the CLI's bookkeeping and not part of what it draws back.
fn is_message_record(line: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Head<'line> {
        #[serde(rename = "type")]
        kind: Option<&'line str>,
        #[serde(rename = "isSidechain", default)]
        sidechain: bool,
        #[serde(default, borrow)]
        message: Option<&'line serde_json::value::RawValue>,
    }
    let Ok(head) = serde_json::from_slice::<Head<'_>>(line) else {
        return false;
    };
    matches!(head.kind, Some("user" | "assistant")) && head.message.is_some() && !head.sidechain
}

/// One stored conversation as the directory listing knows it, before its file is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Indexed {
    native: NativeSessionId,
    modified_ms: u64,
    path: PathBuf,
}

/// Every stored conversation, newest first, with whether the ceiling cut the index short.
struct Index {
    conversations: Vec<Indexed>,
    truncated: bool,
}

/// Read the directory listing: one project directory per folder, one file per conversation.
fn index(provider: ProviderId, projects: &Path) -> Result<Index, ProviderError> {
    let read_failure = |detail: std::io::Error| ProviderError::Protocol {
        provider,
        doing: "reading the conversations this CLI has stored",
        detail: detail.to_string(),
    };
    let project_directories = match fs::read_dir(projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Index {
                conversations: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(read_failure(error)),
    };
    let mut conversations = Vec::new();
    let mut truncated = false;
    'projects: for project in project_directories {
        let project = project.map_err(read_failure)?;
        if !project.file_type().map_err(read_failure)?.is_dir() {
            continue;
        }
        for file in fs::read_dir(project.path()).map_err(read_failure)? {
            let file = file.map_err(read_failure)?;
            let path = file.path();
            if path.extension().and_then(|extension| extension.to_str())
                != Some(CONVERSATION_EXTENSION)
            {
                continue;
            }
            // The conversation's identity is its file name. A name this catalogue cannot carry is not a
            // conversation this surface can offer, and is left where it is.
            let Some(Ok(native)) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(NativeSessionId::new)
            else {
                continue;
            };
            let metadata = file.metadata().map_err(read_failure)?;
            if !metadata.is_file() {
                continue;
            }
            let modified_ms = modified_ms_of(&metadata);
            if conversations.len() >= MAX_INDEXED {
                truncated = true;
                break 'projects;
            }
            conversations.push(Indexed {
                native,
                modified_ms,
                path,
            });
        }
    }
    conversations.sort_by(|left, right| {
        Reverse(left.modified_ms)
            .cmp(&Reverse(right.modified_ms))
            .then_with(|| left.native.as_str().cmp(right.native.as_str()))
    });
    Ok(Index {
        conversations,
        truncated,
    })
}

/// The file's last write as milliseconds since the epoch.
///
/// A platform that reports no modification time, or one before the epoch, puts the conversation at the oldest
/// end rather than out of the list: the row is still real, only its place is unknown.
fn modified_ms_of(metadata: &fs::Metadata) -> u64 {
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) else {
        return 0;
    };
    u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
}

/// A place in the newest-first order, which is what a cursor names.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    modified_ms: u64,
    native: Box<str>,
}

impl Position {
    fn of(indexed: &Indexed) -> Self {
        Self {
            modified_ms: indexed.modified_ms,
            native: indexed.native.as_str().into(),
        }
    }

    fn encode(&self) -> Box<str> {
        format!("{}:{}", self.modified_ms, self.native).into()
    }

    fn decode(provider: ProviderId, cursor: &str) -> Result<Self, ProviderError> {
        let failure = || ProviderError::Protocol {
            provider,
            doing: "reading a pagination cursor for the conversations this CLI has stored",
            detail: "the cursor is not one this store issued".to_owned(),
        };
        let (modified, native) = cursor.split_once(':').ok_or_else(failure)?;
        let modified_ms = modified.parse::<u64>().map_err(|_| failure())?;
        if native.is_empty() || NativeSessionId::new(native).is_err() {
            return Err(failure());
        }
        Ok(Self {
            modified_ms,
            native: native.into(),
        })
    }

    /// Whether `indexed` comes after this position in the newest-first order.
    fn precedes(&self, indexed: &Indexed) -> bool {
        indexed.modified_ms < self.modified_ms
            || indexed.modified_ms == self.modified_ms
                && indexed.native.as_str() > self.native.as_ref()
    }
}

/// Fill one page from the index, opening only the files the page needs.
fn page(
    index: &Index,
    display_titles: &HashMap<String, String>,
    after: Option<&Position>,
    query: &NativeSessionQuery,
    resumable: bool,
) -> NativeSessionCatalogue {
    // At least one row per page, else a page could never advance its cursor.
    let capacity = usize::from(query.limit).clamp(1, MAX_NATIVE_SESSION_ITEMS);
    let root = query.root.as_ref().map(runtrol_provider::AbsPath::as_str);
    let mut sessions = Vec::new();
    let mut next_cursor = None;
    let mut unplaced = 0_usize;
    let mut unreadable = 0_usize;
    let candidates = index
        .conversations
        .iter()
        .filter(|indexed| after.is_none_or(|after| after.precedes(indexed)));
    let mut candidates = candidates.peekable();
    while let Some(indexed) = candidates.next() {
        let Ok(head) = head(&indexed.path) else {
            // A file the CLI holds open or a permission this process lacks: the row is counted and the page
            // says so, rather than one unreadable file hiding every other conversation.
            unreadable = unreadable.saturating_add(1);
            continue;
        };
        let Some(cwd) = head.cwd else {
            unplaced = unplaced.saturating_add(1);
            continue;
        };
        if root.is_some_and(|root| !under(&cwd, root)) {
            continue;
        }
        sessions.push(NativeSessionEntry {
            native: indexed.native.clone(),
            cwd: cwd.into(),
            // One folder per conversation is what the record names. Claiming more would be inventing authority.
            additional_directories: Vec::new(),
            title: head
                .title
                .or_else(|| display_titles.get(indexed.native.as_str()).cloned())
                .map(Into::into),
            // Milliseconds since the epoch, as the CLI's own roster spells a time; the surface reads the digits.
            updated_at: Some(indexed.modified_ms.to_string().into()),
            resume: if resumable {
                NativeResumeCapability::Available
            } else {
                NativeResumeCapability::Unknown
            },
        });
        if sessions.len() >= capacity {
            // A candidate left behind is exactly what a next page is made of.
            if candidates.peek().is_some() {
                next_cursor = Some(Position::of(indexed).encode());
            }
            break;
        }
    }
    let mut limits = Vec::new();
    if unplaced > 0 {
        limits.push("some stored conversations name no folder and are not shown");
    }
    if unreadable > 0 {
        limits.push("some stored conversations could not be read and are not shown");
    }
    if index.truncated {
        limits.push("the store holds more conversations than one listing indexes");
    }
    NativeSessionCatalogue {
        coverage: if limits.is_empty() {
            NativeCatalogueCoverage::Complete {
                source: NativeCatalogueSource::ProviderStore,
            }
        } else {
            NativeCatalogueCoverage::Partial {
                source: NativeCatalogueSource::ProviderStore,
                why: limits.join("; ").into(),
            }
        },
        sessions,
        next_cursor,
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    session_id: Option<String>,
    display: Option<String>,
}

/// Read the provider's own human-facing prompt previews and keep the first one that reads as a request for each
/// conversation.
///
/// This is not a transcript scan. `history.jsonl` is the CLI's structured prompt-history surface, and serde
/// streams past fields such as pasted content without retaining them. The map lives for one catalogue call and
/// is dropped after its labels are attached to the returned page. A leading slash command is passed over so the
/// name is the first thing the person actually asked for, not the CLI control line they happened to type first.
fn history_titles(path: &Path) -> HashMap<String, String> {
    let Ok(file) = File::open(path) else {
        return HashMap::new();
    };
    let entries =
        serde_json::Deserializer::from_reader(BufReader::new(file)).into_iter::<HistoryEntry>();
    // Per conversation, the first prompt that reads as a request. A leading slash command (`/model`, `/clear`) is
    // the CLI's own control line rather than something the person said about the work, so a later substantive
    // prompt is preferred over it; a conversation that only ever ran commands keeps its first command rather than
    // going nameless. The bool records whether the kept title is still such a command placeholder.
    let mut titles: HashMap<String, (String, bool)> = HashMap::new();
    for entry in entries.take(MAX_HISTORY_ROWS) {
        let Ok(entry) = entry else {
            break;
        };
        let (Some(session_id), Some(display)) = (entry.session_id, entry.display) else {
            continue;
        };
        if NativeSessionId::new(&session_id).is_err() {
            continue;
        }
        if titles
            .get(&session_id)
            .is_some_and(|(_, is_command)| !is_command)
        {
            continue;
        }
        let Some(line) = display.lines().find(|line| !line.trim().is_empty()) else {
            continue;
        };
        let line = line.trim();
        if line.chars().any(char::is_control) {
            continue;
        }
        let title = bounded(line);
        if title.is_empty() {
            continue;
        }
        let is_command = title.starts_with('/');
        match titles.get(&session_id) {
            None => {
                titles.insert(session_id, (title, is_command));
            }
            Some((_, true)) if !is_command => {
                titles.insert(session_id, (title, false));
            }
            _ => {}
        }
    }
    titles
        .into_iter()
        .map(|(session_id, (title, _))| (session_id, title))
        .collect()
}

/// What the head of one stored conversation names.
#[derive(Debug, Default, PartialEq, Eq)]
struct Head {
    cwd: Option<String>,
    title: Option<String>,
}

/// Scan one file, one window at a time, and pick out the folder and the title.
///
/// The window carries the last `MAX_VALUE_BYTES` plus a key's length from one step to the next, so a key or a
/// value split across two reads is still seen whole, and the memory held is the same for a 300-byte file and
/// a 44 MB one. Reading stops as soon as both answers are settled, at the folder budget, or at the end.
fn head(path: &Path) -> std::io::Result<Head> {
    let mut file = File::open(path)?;
    let mut window: Vec<u8> = Vec::with_capacity(STEP + CARRY);
    let mut folder = Scan::Absent;
    let mut title = Scan::Absent;
    let mut consumed: u64 = 0;
    loop {
        let read = (&mut file).take(STEP as u64).read_to_end(&mut window)?;
        consumed = consumed.saturating_add(read as u64);
        if !folder.is_settled() {
            folder = scan(&window, FOLDER_KEY);
        }
        if !title.is_settled() && consumed <= TITLE_BUDGET {
            title = scan(&window, TITLE_KEY);
        }
        let title_settled = title.is_settled() || consumed >= TITLE_BUDGET;
        if (folder.is_settled() && title_settled) || read == 0 || consumed >= FOLDER_BUDGET {
            break;
        }
        let keep = window.len().min(CARRY);
        let drop_until = window.len() - keep;
        window.drain(..drop_until);
    }
    let mut head = Head::default();
    if let Scan::Value(cwd) = folder {
        head.cwd = Some(cwd).filter(|cwd| !cwd.is_empty());
    }
    if let Scan::Value(found) = title {
        let found = bounded(found.trim());
        head.title = Some(found).filter(|found| !found.is_empty());
    }
    Ok(head)
}

/// Bytes carried from one window to the next: a value that may still be open, and a key that may be split.
const CARRY: usize = MAX_VALUE_BYTES + TITLE_KEY.len();

/// What a scan for one key found in the window so far.
#[derive(Debug, PartialEq, Eq)]
enum Scan {
    /// The key has not appeared.
    Absent,
    /// The key appeared and its string runs past the window; the next window will see it whole.
    Incomplete,
    /// The key appeared with a string this reader could not decode, or one longer than a folder or a title can
    /// be; treated as no value, and the scan stops looking.
    Undecodable,
    /// The key's string, decoded.
    Value(String),
}

impl Scan {
    const fn is_settled(&self) -> bool {
        matches!(self, Self::Undecodable | Self::Value(_))
    }
}

/// Find the first structural occurrence of `key` in the window and decode the JSON string that follows it.
fn scan(window: &[u8], key: &[u8]) -> Scan {
    let Some(at) = find(window, key) else {
        return Scan::Absent;
    };
    // The key ends with the opening quote of its value; the decoder wants the quotes included.
    let open = at + key.len() - 1;
    let mut cursor = open + 1;
    while let Some(byte) = window.get(cursor) {
        match byte {
            b'\\' => cursor += 2,
            b'"' => {
                let Some(quoted) = window.get(open..=cursor) else {
                    return Scan::Incomplete;
                };
                return match serde_json::from_slice::<String>(quoted) {
                    Ok(value) => Scan::Value(value),
                    Err(_) => Scan::Undecodable,
                };
            }
            _ => cursor += 1,
        }
    }
    if window.len() - open > MAX_VALUE_BYTES {
        return Scan::Undecodable;
    }
    Scan::Incomplete
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use runtrol_provider::AbsPath;

    use super::*;

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let serial = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-claude-store-{}-{name}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("the scratch store is created");
            Self(path)
        }

        /// One stored conversation, written as the CLI writes one, with the given records.
        fn conversation(
            &self,
            slug: &str,
            id: &str,
            lines: &[String],
            modified_ms: u64,
        ) -> PathBuf {
            let directory = self.0.join(slug);
            fs::create_dir_all(&directory).expect("the project directory is created");
            let path = directory.join(format!("{id}.{CONVERSATION_EXTENSION}"));
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .expect("the conversation file is created");
            for line in lines {
                writeln!(file, "{line}").expect("a record is written");
            }
            file.set_modified(UNIX_EPOCH + Duration::from_millis(modified_ms))
                .expect("the modification time is set");
            path
        }

        fn history(&self, lines: &[String]) -> PathBuf {
            let path = self.0.join(HISTORY_FILE);
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .expect("the prompt history is created");
            for line in lines {
                writeln!(file, "{line}").expect("a prompt history row is written");
            }
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn user_record(cwd: &str, text: &str) -> String {
        // The real shape, shortened: the folder comes after the message, as the CLI writes it.
        format!(
            r#"{{"parentUuid":null,"type":"user","message":{{"role":"user","content":[{{"type":"text","text":{text}}}]}},"uuid":"u1","cwd":{cwd},"sessionId":"s"}}"#,
            text = serde_json::to_string(text).expect("text encodes"),
            cwd = serde_json::to_string(cwd).expect("cwd encodes"),
        )
    }

    fn title_record(title: &str) -> String {
        format!(
            r#"{{"type":"ai-title","aiTitle":{title},"sessionId":"s"}}"#,
            title = serde_json::to_string(title).expect("title encodes"),
        )
    }

    fn history_record(session: Option<&str>, display: &str) -> String {
        serde_json::json!({
            "display": display,
            "pastedContents": { "ignored": "provider-owned content is not retained" },
            "sessionId": session,
            "timestamp": 1,
        })
        .to_string()
    }

    #[test]
    fn structured_prompt_history_names_a_conversation_without_an_explicit_title() {
        let scratch = Scratch::new("history-title");
        let path = scratch.history(&[
            history_record(None, "not attached to a conversation"),
            history_record(Some(ALPHA), "  First provider display\nmore detail  "),
            history_record(
                Some(ALPHA),
                "a later prompt does not rename the conversation",
            ),
            history_record(Some(""), "invalid identity"),
        ]);

        let titles = history_titles(&path);

        assert_eq!(titles.len(), 1);
        assert_eq!(
            titles.get(ALPHA).map(String::as_str),
            Some("First provider display")
        );
    }

    #[test]
    fn a_leading_slash_command_is_passed_over_for_the_first_real_request() {
        // The first things typed were `/model` and `/clear`, the CLI's own control lines. A conversation named
        // `/model` tells the operator nothing, so the first prompt that reads as a request becomes the title.
        let scratch = Scratch::new("history-title-command");
        let path = scratch.history(&[
            history_record(Some(ALPHA), "/model"),
            history_record(Some(ALPHA), "/clear"),
            history_record(Some(ALPHA), "wire the usage panel"),
            history_record(Some(ALPHA), "and the icons too"),
        ]);

        let titles = history_titles(&path);

        assert_eq!(
            titles.get(ALPHA).map(String::as_str),
            Some("wire the usage panel")
        );
    }

    #[test]
    fn a_conversation_that_only_ran_commands_keeps_its_first_command_rather_than_going_nameless() {
        // Passing over commands must not erase the name entirely: a conversation that only ever ran commands is
        // still better shown by its first command than by nothing at all.
        let scratch = Scratch::new("history-title-only-commands");
        let path = scratch.history(&[
            history_record(Some(BETA), "/model"),
            history_record(Some(BETA), "/clear"),
        ]);

        let titles = history_titles(&path);

        assert_eq!(titles.get(BETA).map(String::as_str), Some("/model"));
    }

    #[test]
    fn deleting_moves_the_conversation_and_its_sidecar_into_the_reversible_trash() {
        let scratch = Scratch::new("delete");
        let projects = scratch.0.join("projects");
        let folder = projects.join("some-folder");
        fs::create_dir_all(&folder).expect("the project folder is created");
        let file = folder.join(format!("{ALPHA}.{CONVERSATION_EXTENSION}"));
        fs::write(&file, b"{\"type\":\"user\"}\n").expect("the conversation file is written");
        let sidecar = folder.join(ALPHA);
        fs::create_dir_all(&sidecar).expect("the sidecar directory is created");
        fs::write(sidecar.join("agent-1.jsonl"), b"{}\n").expect("a sidecar record is written");

        ClaudeStore::at(projects.clone())
            .delete(ALPHA)
            .expect("the conversation is deleted");

        // Gone from the store, so gone from every listing.
        assert!(!file.exists(), "the conversation file left the store");
        assert!(!sidecar.exists(), "the sidecar directory left the store");
        // Kept in the trash, a sibling of projects, so it can be restored by hand.
        let trash = scratch.0.join("runtrol-deleted");
        assert!(
            trash
                .join(format!("{ALPHA}.{CONVERSATION_EXTENSION}"))
                .is_file(),
            "the conversation file is in the trash"
        );
        assert!(
            trash.join(ALPHA).is_dir(),
            "the sidecar directory is in the trash"
        );

        // Deleting again, with the conversation already gone, is the asked-for state and succeeds.
        ClaudeStore::at(projects)
            .delete(ALPHA)
            .expect("a second delete of a gone conversation succeeds");
    }

    fn query(root: Option<&str>, cursor: Option<&str>) -> NativeSessionQuery {
        NativeSessionQuery {
            root: root.map(|root| AbsPath::new(root).expect("a valid test root")),
            cursor: cursor.map(Into::into),
            limit: 100,
        }
    }

    fn absolute_test_path(tail: &str) -> String {
        if cfg!(windows) {
            format!("C:/{tail}")
        } else {
            format!("/{tail}")
        }
    }

    fn provider() -> ProviderId {
        ProviderId::parse("claude").expect("a valid id")
    }

    fn first_row(catalogue: &NativeSessionCatalogue) -> &NativeSessionEntry {
        catalogue
            .sessions
            .first()
            .expect("the listing has at least one row")
    }

    const ALPHA: &str = "7c2e1d1a-0000-4000-8000-000000000001";
    const BETA: &str = "7c2e1d1a-0000-4000-8000-000000000002";
    const GAMMA: &str = "7c2e1d1a-0000-4000-8000-000000000003";

    #[test]
    fn a_stored_conversation_becomes_a_row_with_its_folder_and_the_cli_own_title() {
        let scratch = Scratch::new("row");
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[
                r#"{"type":"agent-setting","agentSetting":"x","sessionId":"s"}"#.to_owned(),
                user_record("C:\\work\\alpha", "hello"),
                title_record("Greeting the folder"),
            ],
            1_700_000_000_000,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(catalogue.sessions.len(), 1);
        let row = first_row(&catalogue);
        assert_eq!(row.native.as_str(), ALPHA);
        assert_eq!(row.cwd.as_ref(), "C:\\work\\alpha");
        assert_eq!(row.title.as_deref(), Some("Greeting the folder"));
        assert_eq!(row.updated_at.as_deref(), Some("1700000000000"));
        assert_eq!(row.resume, NativeResumeCapability::Available);
        assert_eq!(
            catalogue.coverage,
            NativeCatalogueCoverage::Complete {
                source: NativeCatalogueSource::ProviderStore
            }
        );
    }

    #[test]
    fn a_message_quoting_the_folder_key_does_not_name_the_folder() {
        // A key spelled with unescaped quotes only occurs at a structural position. Inside the message the
        // quotes are escaped, so the record's own folder, which comes after the message, is the one found.
        let scratch = Scratch::new("quoted");
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[user_record(
                "C:\\work\\alpha",
                r#"look at this record: {"cwd":"D:\\elsewhere"}"#,
            )],
            1,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(first_row(&catalogue).cwd.as_ref(), "C:\\work\\alpha");
    }

    #[test]
    fn a_folder_and_a_title_behind_a_long_first_message_are_still_found() {
        // Measured on the machine that built this: one first message was a 1.08 MB paste, and the folder sat
        // right behind it. The window rolls; it does not give up at a fixed prefix.
        let scratch = Scratch::new("deep");
        let long = "x".repeat(STEP * 20);
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[
                user_record("C:\\work\\alpha", &long),
                title_record("Late title"),
            ],
            1,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(first_row(&catalogue).cwd.as_ref(), "C:\\work\\alpha");
        assert_eq!(first_row(&catalogue).title.as_deref(), Some("Late title"));
    }

    #[test]
    fn a_conversation_naming_no_folder_is_counted_and_said() {
        // Measured: a renamed conversation nobody wrote in holds a title and a name and no message, so no
        // record names a folder. It is not placed, and the page says so.
        let scratch = Scratch::new("unplaced");
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[
                title_record("Renamed, never used"),
                r#"{"type":"agent-name","agentName":"Renamed, never used","sessionId":"s"}"#
                    .to_owned(),
            ],
            2,
        );
        scratch.conversation(
            "C--work-alpha",
            BETA,
            &[user_record("C:\\work\\alpha", "short")],
            1,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(catalogue.sessions.len(), 1);
        assert_eq!(first_row(&catalogue).native.as_str(), BETA);
        assert!(
            matches!(
                &catalogue.coverage,
                NativeCatalogueCoverage::Partial { source: NativeCatalogueSource::ProviderStore, why }
                    if why.contains("name no folder")
            ),
            "{:?}",
            catalogue.coverage
        );
    }

    #[test]
    fn newest_first_and_paged_by_a_cursor_this_store_issued() {
        let scratch = Scratch::new("paged");
        for (id, at) in [(ALPHA, 10), (BETA, 30), (GAMMA, 20)] {
            scratch.conversation(
                "C--work-alpha",
                id,
                &[user_record("C:\\work\\alpha", "hi")],
                at,
            );
        }
        let store = ClaudeStore::at(scratch.0.clone());
        let mut first_query = query(None, None);
        first_query.limit = 2;
        let first = store
            .list(provider(), true, &first_query)
            .expect("the first page lists");
        let ids = |catalogue: &NativeSessionCatalogue| {
            catalogue
                .sessions
                .iter()
                .map(|row| row.native.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first), vec![BETA.to_owned(), GAMMA.to_owned()]);
        let cursor = first
            .next_cursor
            .clone()
            .expect("a third conversation remains");
        let mut second_query = query(None, Some(&cursor));
        second_query.limit = 2;
        let second = store
            .list(provider(), true, &second_query)
            .expect("the second page lists");
        assert_eq!(ids(&second), vec![ALPHA.to_owned()]);
        assert!(second.next_cursor.is_none(), "the last page has no next");

        let forged = store.list(provider(), true, &query(None, Some("not a cursor")));
        assert!(matches!(forged, Err(ProviderError::Protocol { .. })));
    }

    #[test]
    fn a_page_that_fills_on_the_last_conversation_has_no_next_page() {
        let scratch = Scratch::new("exact");
        for (id, at) in [(ALPHA, 10), (BETA, 30)] {
            scratch.conversation(
                "C--work-alpha",
                id,
                &[user_record("C:\\work\\alpha", "hi")],
                at,
            );
        }
        let mut exact = query(None, None);
        exact.limit = 2;
        let page = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &exact)
            .expect("the page lists");
        assert_eq!(page.sessions.len(), 2);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn a_folder_query_keeps_only_its_own_conversations_however_the_cli_spelled_the_folder() {
        let scratch = Scratch::new("folder");
        let root = absolute_test_path("work/alpha");
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[user_record(&absolute_test_path("WORK\\alpha"), "hi")],
            3,
        );
        scratch.conversation(
            "C--work-alpha-other",
            BETA,
            &[user_record(&absolute_test_path("work/alpha-other"), "hi")],
            2,
        );
        scratch.conversation(
            "C--work-alpha-nested",
            GAMMA,
            &[user_record(&format!("{root}\\nested"), "hi")],
            1,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(Some(&root), None))
            .expect("the store lists");
        let ids = catalogue
            .sessions
            .iter()
            .map(|row| row.native.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![ALPHA, GAMMA]);
    }

    #[test]
    fn only_conversation_files_of_the_projects_directory_are_conversations() {
        let scratch = Scratch::new("shape");
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[user_record("C:\\work\\alpha", "hi")],
            1,
        );
        // A sub-agent transcript lives one level deeper and is that conversation's, not a conversation.
        let nested = scratch
            .0
            .join("C--work-alpha")
            .join(ALPHA)
            .join("subagents");
        fs::create_dir_all(&nested).expect("nested directory");
        fs::write(
            nested.join("agent-1.jsonl"),
            user_record("C:\\work\\alpha", "nested"),
        )
        .expect("nested file");
        // And a file of another kind beside the conversations is the CLI's own business.
        fs::write(scratch.0.join("C--work-alpha").join("notes.json"), "{}").expect("other file");
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(catalogue.sessions.len(), 1);
    }

    #[test]
    fn a_store_that_does_not_exist_yet_is_empty_and_complete() {
        let scratch = Scratch::new("fresh");
        let catalogue = ClaudeStore::at(scratch.0.join("never-created"))
            .list(provider(), true, &query(None, None))
            .expect("an absent store lists nothing");
        assert!(catalogue.sessions.is_empty());
        assert_eq!(
            catalogue.coverage,
            NativeCatalogueCoverage::Complete {
                source: NativeCatalogueSource::ProviderStore
            }
        );
    }

    #[test]
    fn a_store_that_cannot_be_located_says_so_without_rows() {
        let store = ClaudeStore {
            projects: Err(HomeProblem::Missing),
        };
        let catalogue = store
            .list(provider(), true, &query(None, None))
            .expect("an unlocated store is an answer");
        assert!(catalogue.sessions.is_empty());
        assert!(matches!(
            catalogue.coverage,
            NativeCatalogueCoverage::Unsupported { .. }
        ));
    }

    #[test]
    fn a_resume_replays_the_conversation_tail_as_the_cli_wrote_it() {
        let scratch = Scratch::new("replay");
        let assistant = |text: &str| {
            format!(
                r#"{{"parentUuid":"u","isSidechain":false,"type":"assistant","message":{{"id":"msg_1","role":"assistant","content":[{{"type":"text","text":{text}}}]}},"uuid":"a1","cwd":"C:\\work\\alpha","sessionId":"s"}}"#,
                text = serde_json::to_string(text).expect("text encodes"),
            )
        };
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[
                r#"{"type":"agent-setting","agentSetting":"x","sessionId":"s"}"#.to_owned(),
                user_record("C:\\work\\alpha", "first question"),
                assistant("first answer"),
                title_record("A title, not a message"),
                r#"{"parentUuid":"a1","isSidechain":true,"type":"user","message":{"role":"user","content":"sub-agent prompt"},"uuid":"side","sessionId":"s"}"#.to_owned(),
                user_record("C:\\work\\alpha", "second question"),
                r#"{"type":"last-prompt","lastPrompt":"second question","sessionId":"s"}"#.to_owned(),
            ],
            1,
        );
        let replay = ClaudeStore::at(scratch.0.clone()).recent_records(ALPHA);
        assert_eq!(replay.problem, None);
        let texts: Vec<String> = replay
            .records
            .iter()
            .map(|record| String::from_utf8_lossy(record).into_owned())
            .collect();
        let joined = texts.join(
            "
",
        );
        assert_eq!(texts.len(), 3, "{texts:?}");
        for (index, expected) in ["first question", "first answer", "second question"]
            .into_iter()
            .enumerate()
        {
            assert!(
                texts.get(index).is_some_and(|text| text.contains(expected)),
                "record {index} should carry {expected:?}: {joined}"
            );
        }
        assert!(
            !texts.iter().any(|text| text.contains("sub-agent prompt")),
            "a sidechain is the sub-agent's, not the conversation's"
        );
    }

    #[test]
    fn a_resume_replays_only_the_bounded_tail_and_drops_the_cut_record() {
        // The tail is measured in bytes and starts mid-record; that fragment is dropped and the whole records
        // after it are kept, the newest `MAX_REPLAY_RECORDS` of them.
        let scratch = Scratch::new("replay-tail");
        let filler = "x".repeat(1024);
        let mut lines = Vec::new();
        let total = MAX_REPLAY_RECORDS + 40;
        for index in 0..total {
            lines.push(user_record(
                "C:\\work\\alpha",
                &format!("{filler} q{index}"),
            ));
        }
        // A huge early record, larger than the tail budget, so the tail starts inside it.
        let huge = "y".repeat(usize::try_from(REPLAY_TAIL_BYTES).expect("fits") + 4096);
        lines.insert(1, user_record("C:\\work\\alpha", &huge));
        scratch.conversation("C--work-alpha", ALPHA, &lines, 1);
        let replay = ClaudeStore::at(scratch.0.clone()).recent_records(ALPHA);
        assert!(
            replay.records.len() <= MAX_REPLAY_RECORDS,
            "{} records",
            replay.records.len()
        );
        assert!(
            replay.records.iter().map(Bytes::len).sum::<usize>() <= MAX_REPLAY_BYTES,
            "the byte bound holds as well"
        );
        let last = String::from_utf8_lossy(replay.records.last().expect("a record"));
        assert!(last.contains(&format!("q{}", total - 1)));
        assert!(
            !replay
                .records
                .iter()
                .any(|record| record.len() > 2 * 1024 * 1024),
            "the cut record is never replayed as a fragment"
        );
    }

    #[test]
    fn a_resume_tail_opens_at_a_prompt_so_the_exchange_reads_whole() {
        // The byte cut would otherwise leave a tool result whose call was cut in front, and the page draws a
        // result only under its call. The tail starts at the newest prompt that fits.
        let scratch = Scratch::new("replay-prompt-start");
        let big = "z".repeat(20 * 1024);
        let tool_result = |index: usize| {
            format!(
                r#"{{"parentUuid":"a","isSidechain":false,"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{index}","content":"{big}"}}]}},"uuid":"r{index}","cwd":"C:\\work\\alpha","sessionId":"s","toolUseResult":{{"stdout":"x"}}}}"#
            )
        };
        let lines = vec![
            user_record("C:\\work\\alpha", &format!("{big} q0")),
            tool_result(0),
            tool_result(1),
            user_record("C:\\work\\alpha", "q1 short"),
            tool_result(2),
        ];
        scratch.conversation("C--work-alpha", ALPHA, &lines, 1);
        let replay = ClaudeStore::at(scratch.0.clone()).recent_records(ALPHA);
        let first = String::from_utf8_lossy(replay.records.first().expect("a record"));
        assert!(
            first.contains("q1 short"),
            "the tail opens at the newest prompt that fits: {first}"
        );
        assert_eq!(replay.records.len(), 2);
    }

    #[test]
    fn a_resume_replays_no_more_bytes_than_a_late_subscriber_would_see() {
        let scratch = Scratch::new("replay-bytes");
        let big = "z".repeat(20 * 1024);
        let lines: Vec<String> = (0..5)
            .map(|index| user_record("C:\\work\\alpha", &format!("{big} q{index}")))
            .collect();
        scratch.conversation("C--work-alpha", ALPHA, &lines, 1);
        let replay = ClaudeStore::at(scratch.0.clone()).recent_records(ALPHA);
        let total: usize = replay.records.iter().map(Bytes::len).sum();
        assert!(total <= MAX_REPLAY_BYTES, "{total} bytes replayed");
        assert_eq!(replay.records.len(), 2, "the newest two fit");
        let last = String::from_utf8_lossy(replay.records.last().expect("a record"));
        assert!(last.contains("q4"));
    }

    #[test]
    fn a_conversation_the_store_does_not_hold_replays_nothing() {
        let scratch = Scratch::new("replay-none");
        assert_eq!(
            ClaudeStore::at(scratch.0.clone()).recent_records(BETA),
            Replay::default()
        );
        assert_eq!(
            ClaudeStore::at(scratch.0.clone()).recent_records("../escape"),
            Replay::default()
        );
        assert_eq!(
            ClaudeStore {
                projects: Err(HomeProblem::Missing),
            }
            .recent_records(ALPHA),
            Replay::default()
        );
    }

    #[test]
    fn the_scan_reads_escapes_as_json_does() {
        let buffer = br#"{"cwd":"C:\\Users\\x \"q\" \u00e9","next":1}"#;
        assert_eq!(
            scan(buffer, FOLDER_KEY),
            Scan::Value("C:\\Users\\x \"q\" \u{e9}".to_owned())
        );
        assert_eq!(
            scan(br#"{"cwd":"unterminated"#, FOLDER_KEY),
            Scan::Incomplete
        );
        assert_eq!(scan(br#"{"other":"x"}"#, FOLDER_KEY), Scan::Absent);
    }

    #[test]
    fn a_value_longer_than_any_folder_or_title_is_not_carried_forever() {
        let mut window = br#"{"cwd":""#.to_vec();
        window.extend(std::iter::repeat_n(b'x', MAX_VALUE_BYTES + 1));
        assert_eq!(scan(&window, FOLDER_KEY), Scan::Undecodable);
    }

    #[test]
    fn a_key_split_across_two_windows_is_still_seen_whole() {
        // The carry keeps a key's length from the previous window, so a key that straddles a read boundary
        // is found by the next scan. Built to straddle: a filler that ends one byte into the key.
        let scratch = Scratch::new("straddle");
        let filler = "y".repeat(STEP - 2);
        scratch.conversation(
            "C--work-alpha",
            ALPHA,
            &[format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{filler}"}}]}},"cwd":"C:\\work\\alpha"}}"#
            )],
            1,
        );
        let catalogue = ClaudeStore::at(scratch.0.clone())
            .list(provider(), true, &query(None, None))
            .expect("the store lists");
        assert_eq!(first_row(&catalogue).cwd.as_ref(), "C:\\work\\alpha");
    }
}
