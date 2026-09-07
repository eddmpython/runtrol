//! Structural turn and title metadata from the provider's original session file.
//!
//! No message is retained or interpreted. Model proof uses only direct record/message metadata and its
//! timestamp relative to the exact process birth. Filesystem stamps invalidate reads, never model state.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use runtrol_provider::{NativeSessionId, ProcessIdentity, WallMs};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

use crate::native_turn::NativeTurn;

use super::store::PROJECTS_DIRECTORY;
const TRANSCRIPT_EXTENSION: &str = "jsonl";
const MAX_WINDOW_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PROJECT_DIRECTORIES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl FileStamp {
    fn read(metadata: &fs::Metadata) -> Self {
        let (created, modified) = crate::native_watch::source_times(metadata);
        Self {
            size: metadata.len(),
            // Unsupported timestamps only disable that invalidation key. They never imply idle.
            modified,
            created,
        }
    }
}

#[derive(Debug)]
struct Followed {
    transcript: PathBuf,
    stamp: FileStamp,
    read_to: u64,
    read_turns: bool,
    metadata: Metadata,
}

#[derive(Clone, Copy, Debug, Default)]
struct Metadata {
    boundary: Option<NativeTurn>,
    title: Option<[u8; 32]>,
    folder: Option<[u8; 32]>,
}

#[derive(Debug)]
enum Known {
    Found(Followed),
    NotYet(Option<runtrol_childproc::watch::DirectoryChanges>),
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MetadataObservation {
    pub(super) model: Option<bool>,
    pub(super) revision: [u8; 32],
    pub(super) unavailable: bool,
}

#[derive(Clone, Debug)]
pub(super) struct TranscriptActivity {
    projects: Option<PathBuf>,
    seen: Arc<Mutex<HashMap<Box<str>, Known>>>,
}

impl TranscriptActivity {
    pub(super) fn new(config: Option<PathBuf>) -> Self {
        Self {
            projects: config.map(|config| config.join(PROJECTS_DIRECTORY)),
            seen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    fn rooted(projects: PathBuf, _now: fn() -> SystemTime) -> Self {
        Self {
            projects: Some(projects),
            seen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    fn answering(&self, session: &str) -> bool {
        self.read_metadata(session, true)
            .unwrap_or_default()
            .and_then(|metadata| metadata.boundary)
            .is_some_and(|boundary| boundary.answering)
    }

    pub(super) fn retain(&self, live: &BTreeSet<NativeSessionId>) {
        self.seen
            .blocking_lock()
            .retain(|native, _| live.iter().any(|live| live.as_str() == native.as_ref()));
    }

    pub(super) fn observation(
        &self,
        session: &str,
        owner: Option<ProcessIdentity>,
        read_turns: bool,
    ) -> MetadataObservation {
        let (metadata, unavailable) = match self.read_metadata(session, read_turns) {
            Ok(metadata) => (metadata, false),
            Err(()) => (None, true),
        };
        let mut signature = Sha256::new();
        // Creation and the first published folder make a previously unlisted identity available to the catalogue.
        // Neither fact says anything about model work, and later body appends leave this proof unchanged.
        signature.update([u8::from(metadata.is_some())]);
        let Metadata {
            boundary,
            title,
            folder,
        } = metadata.unwrap_or_default();
        if let Some(boundary) = boundary {
            signature.update([u8::from(boundary.answering)]);
            signature.update(boundary.at.map_or(0, WallMs::as_millis).to_le_bytes());
        }
        if let Some(title) = title {
            signature.update(title);
        }
        if let Some(folder) = folder {
            signature.update(folder);
        }
        MetadataObservation {
            model: boundary.and_then(|boundary| boundary.state_for(owner)),
            revision: signature.finalize().into(),
            unavailable,
        }
    }

    fn read_metadata(&self, session: &str, read_turns: bool) -> Result<Option<Metadata>, ()> {
        let Some(projects) = &self.projects else {
            return Ok(None);
        };
        let mut seen = self.seen.blocking_lock();
        let Some(transcript) = cached_transcript(projects, session, &mut seen)? else {
            return Ok(None);
        };
        let Ok(metadata) = fs::metadata(&transcript) else {
            seen.remove(session);
            return Err(());
        };
        let stamp = FileStamp::read(&metadata);
        let previous = match seen.get(session) {
            Some(Known::Found(known)) => Some(known),
            _ => None,
        };
        if let Some(previous) = previous
            && previous.stamp == stamp
            && (!read_turns || previous.read_turns)
        {
            return Ok(Some(previous.metadata));
        }
        let continuing = previous.is_some_and(|previous| {
            previous.stamp.created == stamp.created
                && previous.stamp.size < stamp.size
                && (!read_turns || previous.read_turns)
                && stamp.size.saturating_sub(previous.read_to) <= MAX_WINDOW_BYTES
        });
        let from = if continuing {
            previous.map_or(0, |known| known.read_to)
        } else {
            stamp.size.saturating_sub(MAX_WINDOW_BYTES)
        };
        let Ok(bytes) = read_range(&transcript, from, stamp.size) else {
            // A failed read cannot preserve model proof. Keep no fabricated closed boundary.
            seen.remove(session);
            return Err(());
        };
        let mut metadata = if continuing {
            previous.map_or_else(Metadata::default, |known| known.metadata)
        } else {
            Metadata::default()
        };
        let mut position = 0;
        let skip_fragment = from > 0 && !continuing;
        let mut read_to = from;
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            let complete = position + line.len() < bytes.len();
            if !(skip_fragment && index == 0) {
                if read_turns && let Some(next) = turn_boundary(line) {
                    metadata.boundary = Some(next);
                }
                // The direct aiTitle field is already the catalogue's provider-owned nameplate.
                // Most body records do not contain that key, so they require no JSON walk for titles.
                if (memchr::memmem::find(line, b"\"aiTitle\"").is_some()
                    || metadata.folder.is_none()
                        && memchr::memmem::find(line, b"\"cwd\"").is_some())
                    && let Ok(record) = serde_json::from_slice::<CatalogueRecord<'_>>(line)
                {
                    if let Some(name) = record.ai_title {
                        metadata.title = Some(Sha256::digest(name.as_bytes()).into());
                    }
                    if metadata.folder.is_none()
                        && let Some(folder) = record.cwd
                    {
                        metadata.folder = Some(Sha256::digest(folder.as_bytes()).into());
                    }
                }
            }
            position = position.saturating_add(line.len()).saturating_add(1);
            if complete {
                read_to = from + position as u64;
            }
        }
        seen.insert(
            session.into(),
            Known::Found(Followed {
                transcript,
                stamp,
                read_to,
                read_turns,
                metadata,
            }),
        );
        Ok(Some(metadata))
    }
}

fn cached_transcript(
    projects: &Path,
    session: &str,
    seen: &mut HashMap<Box<str>, Known>,
) -> Result<Option<PathBuf>, ()> {
    if let Some(Known::NotYet(Some(watch))) = seen.get_mut(session)
        && matches!(watch.changed(), Ok(false))
    {
        return Ok(None);
    }
    if let Some(Known::Found(known)) = seen.get(session) {
        return Ok(Some(known.transcript.clone()));
    }
    // Install before searching. Without notification proof, an absence is never reused.
    let mut changes = None;
    if let Ok(watch) = runtrol_childproc::watch::DirectoryChanges::new(projects) {
        changes = Some(watch);
    }
    let (found, complete) = locate_transcript(projects, session);
    if found.is_none() && complete {
        seen.insert(session.into(), Known::NotYet(changes));
    }
    if found.is_some() || complete {
        Ok(found)
    } else {
        Err(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogueRecord<'a> {
    #[serde(borrow)]
    ai_title: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow)]
    cwd: Option<std::borrow::Cow<'a, str>>,
}

#[derive(Deserialize)]
struct TurnRecord<'a> {
    r#type: &'a str,
    timestamp: Option<&'a str>,
    #[serde(borrow)]
    message: Option<&'a serde_json::value::RawValue>,
    #[serde(default, rename = "isSidechain")]
    sidechain: bool,
}

#[derive(Deserialize)]
struct MessageStop<'a> {
    stop_reason: Option<&'a str>,
}

fn turn_boundary(line: &[u8]) -> Option<NativeTurn> {
    let Ok(record) = serde_json::from_slice::<TurnRecord<'_>>(line) else {
        return None;
    };
    if record.sidechain {
        return None;
    }
    let at = record.timestamp.and_then(WallMs::from_iso8601);
    let answering = match record.r#type {
        "user" => true,
        "assistant" => {
            let Ok(message) = serde_json::from_str::<MessageStop<'_>>(record.message?.get()) else {
                return None;
            };
            match message.stop_reason? {
                "tool_use" => true,
                "end_turn" | "stop_sequence" => false,
                // An unknown terminal marker supersedes earlier proof without pretending it ended.
                _ => {
                    return Some(NativeTurn {
                        answering: false,
                        at: None,
                    });
                }
            }
        }
        _ => return None,
    };
    Some(NativeTurn { answering, at })
}

fn locate_transcript(projects: &Path, session: &str) -> (Option<PathBuf>, bool) {
    if session.is_empty() || session.contains(['/', '\\', '.']) {
        return (None, true);
    }
    let entries = match fs::read_dir(projects) {
        Ok(entries) => entries,
        Err(error) => return (None, error.kind() == io::ErrorKind::NotFound),
    };
    let name = format!("{session}.{TRANSCRIPT_EXTENSION}");
    for (index, entry) in entries.enumerate() {
        if index >= MAX_PROJECT_DIRECTORIES {
            return (None, false);
        }
        let Ok(entry) = entry else {
            return (None, false);
        };
        let path = entry.path().join(&name);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => return (Some(path), true),
            Ok(_) => {}
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.kind() == io::ErrorKind::NotADirectory => {}
            Err(_) => return (None, false),
        }
    }
    (None, true)
}

fn read_range(path: &Path, from: u64, to: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    let expected = to.saturating_sub(from).min(MAX_WINDOW_BYTES);
    file.take(expected).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the metadata source changed during its bounded read",
        ));
    }
    Ok(bytes)
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
    fn output_silence_does_not_close_a_structurally_open_turn() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000005";
        scratch.transcript("slug", session, &assistant("tool_use"));
        // Age is not a model state. The same boundary remains open under the later fixture clock.
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), stale);
        assert!(activity.answering(session));
    }

    #[test]
    fn a_session_with_no_transcript_is_not_answering() {
        let scratch = Scratch::new();
        fs::create_dir_all(scratch.root.join(PROJECTS_DIRECTORY)).expect("projects exists");
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(!activity.answering("bbbbbbbb-0000-4000-8000-000000000001"));
    }

    #[test]
    fn first_file_and_delayed_folder_publish_catalogue_revisions_without_model_proof() {
        let scratch = Scratch::new();
        let projects = scratch.root.join(PROJECTS_DIRECTORY);
        fs::create_dir_all(&projects).expect("isolated projects");
        let session = "bbbbbbbb-0000-4000-8000-000000000002";
        let activity = TranscriptActivity::rooted(projects.clone(), RECENT);
        let missing = activity.observation(session, None, false);
        scratch.transcript("slug", session, "");
        let created = activity.observation(session, None, false);
        assert_ne!(missing.revision, created.revision);
        let path = projects.join("slug").join(format!("{session}.jsonl"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("provider source");
        file.write_all(b"{\"body\":{\"cwd\":\"untrusted\"}}\n{\"cwd\":\"C:/fixture")
            .expect("nested field and incomplete direct metadata");
        assert_eq!(
            created.revision,
            activity.observation(session, None, false).revision
        );
        file.write_all(b"\"}\n")
            .expect("provider finishes its folder metadata");
        let placed = activity.observation(session, None, false);
        assert_ne!(created.revision, placed.revision);
        file.write_all(b"{\"body\":\"ordinary output\"}\n")
            .expect("opaque body append");
        let unchanged = activity.observation(session, None, false);
        assert_eq!(placed.revision, unchanged.revision);
        for observed in [missing, created, placed, unchanged] {
            assert_eq!(observed.model, None);
            assert!(!observed.unavailable);
        }
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

    #[test]
    fn the_very_first_turn_is_answering_before_any_marker_exists() {
        // A fresh conversation: setup records and the person's prompt, no stop_reason anywhere yet. The model
        // is generating its first text reply, which writes its only marker at the end (verifier, 2026-08-30).
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000008";
        let body = format!(
            "{{\"type\":\"agent-setting\"}}
{}",
            user()
        );
        scratch.transcript("slug", session, &body);
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(activity.answering(session));
    }

    #[test]
    fn a_transcript_of_setup_records_with_no_prompt_is_not_answering() {
        let scratch = Scratch::new();
        let session = "aaaaaaaa-0000-4000-8000-000000000009";
        scratch.transcript(
            "slug",
            session,
            "{\"type\":\"agent-setting\"}
{\"type\":\"mode\"}
",
        );
        let activity = TranscriptActivity::rooted(scratch.root.join(PROJECTS_DIRECTORY), RECENT);
        assert!(!activity.answering(session));
    }

    #[cfg(windows)]
    fn exact_owner(at: &str) -> ProcessIdentity {
        let millis = WallMs::from_iso8601(at)
            .expect("fixture timestamp")
            .as_millis();
        ProcessIdentity::new(7, (11_644_473_600_000 + millis) * 10_000)
            .expect("fixture exact birth")
    }

    #[cfg(windows)]
    #[test]
    fn model_proof_belongs_to_the_current_incarnation_without_an_idle_clock() {
        let scratch = Scratch::new();
        let session = "cccccccc-0000-4000-8000-000000000001";
        let body = "{\"type\":\"assistant\",\"timestamp\":\"2026-09-07T00:00:01Z\",\"message\":{\"stop_reason\":\"tool_use\"}}\n";
        scratch.transcript("slug", session, body);
        let activity = TranscriptActivity::new(Some(scratch.root.clone()));
        let current = exact_owner("2026-09-07T00:00:00Z");
        let resumed = exact_owner("2026-09-07T00:00:02Z");
        assert_eq!(
            activity.observation(session, Some(current), true).model,
            Some(true)
        );
        assert_eq!(
            activity.observation(session, Some(resumed), true).model,
            None
        );
        assert_eq!(activity.observation(session, None, true).model, None);
        assert_eq!(
            activity.observation(session, Some(current), true).model,
            Some(true)
        );
    }

    #[test]
    fn late_direct_title_changes_revision_while_body_and_nested_title_do_not() {
        let scratch = Scratch::new();
        let session = "cccccccc-0000-4000-8000-000000000002";
        let projects = scratch.transcript("slug", session, "{\"type\":\"setup\"}\n");
        let activity = TranscriptActivity::new(Some(scratch.root.clone()));
        let first = activity.observation(session, None, false);
        let path = projects.join("slug").join(format!("{session}.jsonl"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("fixture source");
        file.write_all(b"{\"type\":\"assistant\",\"message\":{\"aiTitle\":\"untrusted body\",\"stop_reason\":\"end_turn\"}}\n")
            .expect("body fixture");
        assert_eq!(
            activity.observation(session, None, false).revision,
            first.revision
        );
        file.write_all(b"{\"aiTitle\":\"first nameplate\"}\n")
            .expect("title fixture");
        let titled = activity.observation(session, None, false);
        assert_ne!(titled.revision, first.revision);
        assert_eq!(
            activity.observation(session, None, false).revision,
            titled.revision
        );
        file.write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"ordinary output\"}}\n")
            .expect("body append");
        assert_eq!(
            activity.observation(session, None, false).revision,
            titled.revision
        );
        file.write_all(b"{\"aiTitle\":\"later nameplate\"}\n")
            .expect("late title fixture");
        assert_ne!(
            activity.observation(session, None, false).revision,
            titled.revision
        );
        drop(file);
        fs::write(&path, b"{\"type\":\"setup\"}\n").expect("replacement source");
        assert_eq!(
            activity.observation(session, None, false).revision,
            first.revision
        );
    }

    #[cfg(windows)]
    #[test]
    fn modern_roster_title_observation_does_not_read_turn_state() {
        let scratch = Scratch::new();
        let session = "cccccccc-0000-4000-8000-000000000003";
        scratch.transcript(
            "slug",
            session,
            "{\"type\":\"user\",\"timestamp\":\"2026-09-07T00:00:01Z\"}\n",
        );
        let activity = TranscriptActivity::new(Some(scratch.root.clone()));
        let owner = Some(exact_owner("2026-09-07T00:00:00Z"));
        assert_eq!(activity.observation(session, owner, false).model, None);
        assert_eq!(activity.observation(session, owner, true).model, Some(true));
    }

    #[test]
    fn partial_line_is_revisited_and_short_reads_do_not_supply_cached_proof() {
        let scratch = Scratch::new();
        let session = "cccccccc-0000-4000-8000-000000000004";
        let projects =
            scratch.transcript("slug", session, "{\"aiTitle\":\"first\"}\n{\"aiTitle\":\"");
        let activity = TranscriptActivity::new(Some(scratch.root.clone()));
        let first = activity.observation(session, None, false);
        let path = projects.join("slug").join(format!("{session}.jsonl"));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("fixture source");
        file.write_all(b"second\"}\n")
            .expect("complete title fixture");
        let second = activity.observation(session, None, false);
        assert_ne!(second.revision, first.revision);
        assert_eq!(
            read_range(
                &path,
                0,
                fs::metadata(&path).expect("fixture metadata").len() + 1
            )
            .expect_err("short source has no complete read proof")
            .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
