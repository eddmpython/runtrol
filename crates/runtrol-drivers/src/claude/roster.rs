//! Which of this CLI's conversations have a model answering in them right now.
//!
//! # Why the CLI's own roster, and not the transcript's timestamp
//!
//! The first answer to "is a turn running" was the conversation file's modification time: written in the last
//! few seconds meant running. It is wrong in both directions and was measured to be. The CLI writes a record
//! when a message completes, so a turn that spends four minutes inside one command touches nothing for four
//! minutes and reads as idle, while a turn that has just ended keeps reading as running until the window
//! passes. A window wide enough to cover the first mistake makes the second one worse.
//!
//! The CLI already publishes the answer. Every running process of it writes `<config>/sessions/<pid>.json`,
//! and that file names the conversation the process is in and what the process is doing (measured on 2.1.250:
//! `sessionId`, `cwd`, `pid`, a `status` of `busy`, `idle` or `waiting`, and the moment the status last
//! changed). `busy` is a model answering. That is the fact this module reports, in the service's own words.
//!
//! # Why the process is still asked about
//!
//! The file is written when the status changes and never again, and it is not removed when the process ends.
//! Measured on 2.1.250: a conversation continued in a new process left the old process's file behind, still
//! saying `busy` twenty minutes after that process had gone. Believing the file alone leaves a conversation
//! turning forever. So a record counts only while the operating system still holds a process for it.
//!
//! # What this is not
//!
//! Not a transcript read. Nothing here opens a conversation, and the roster carries no message, no prompt and
//! no output. It is the same kind of knowledge as a process list: who is running, and whether they are busy.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use runtrol_provider::{
    NativeProcessActivity, NativeProcessBinding, NativeProcessObservation, NativeSessionId,
    NativeTerminalAccess, NativeTerminalTarget, ProviderError, ProviderId,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::native_watch::NativeSource;

use crate::claude::activity::TranscriptActivity;
use crate::claude::home::{HomeProblem, config_directory};

/// Where the CLI writes one file per running process of itself.
const SESSIONS_DIRECTORY: &str = "sessions";

/// The extension of a roster record. The directory holds the CLI's per-process keys as well, which are not
/// records and are not read.
const RECORD_EXTENSION: &str = "json";

/// The largest a roster record may be. Measured on 2.1.250: 607 bytes at the largest, and a file far past that
/// is not the small record this reads, so it is stepped over rather than parsed.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Maximum directory entries one compatibility observation will inspect.
///
/// A normal provider roster has one small record and one key per live process. Walking an attacker-sized or
/// corrupted directory four times a second would violate both the CPU and latency contracts, so the driver fails
/// the observation and Studio retains its last bounded answer instead.
const MAX_ROSTER_ENTRIES: usize = 1024;

/// What the CLI calls a process of itself while a model is answering in it.
const BUSY: &str = "busy";

/// The CLI's own record of one running process of itself. Only the fields this question needs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The operating system process that wrote this record.
    pid: u32,
    /// Kernel-recorded start value, which prevents a stale roster file from aliasing a reused PID.
    #[serde(default)]
    proc_start: Option<String>,
    /// The conversation that process is in, named the way the stored conversation is named.
    session_id: String,
    /// What the process is doing. Absent in a record written before the CLI had the field.
    #[serde(default)]
    status: Option<String>,
    /// Where the process works. Read so a mirrored terminal can be filed under its folder.
    #[serde(default)]
    cwd: Option<String>,
    /// Provider-owned live peer protocol. Version one publishes the attachment socket used by `claude attach`.
    #[serde(default)]
    peer_protocol: Option<u32>,
    /// Provider-owned endpoint for another official TUI client. Runtime never opens or persists this path; its
    /// presence beside a job identity is part of the provider's proof that `claude attach <job>` is available.
    #[serde(default)]
    messaging_socket_path: Option<String>,
    /// Opaque background job identity accepted by the provider's `attach` and `stop` commands.
    #[serde(default)]
    job_id: Option<String>,
    /// The provider's nameplate and its revision markers, never a transcript field.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    name_since: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    name_source: Option<String>,
    /// Detects a completed turn even when both observations see the same idle state.
    #[serde(default)]
    status_updated_at: Option<Box<serde_json::value::RawValue>>,
}

/// The one honest route into a live terminal session: the provider's own attachment command.
///
/// A record that publishes the complete official target is attachable. Any other live record is observe-only: the
/// console its process may own is never joined (an arbitrary external terminal is focus-only, `PLAN-02`), and the
/// Runtime proves the window that owns the terminal instead.
fn terminal_access(record: &Record) -> NativeTerminalAccess {
    let has_official_peer = record.peer_protocol.is_some_and(|version| version >= 1)
        && record
            .messaging_socket_path
            .as_deref()
            .is_some_and(|path| !path.is_empty());
    if has_official_peer
        && let Some(raw_target) = record.job_id.as_deref()
        && let Ok(target) = NativeTerminalTarget::new(raw_target)
    {
        NativeTerminalAccess::Official { target }
    } else {
        NativeTerminalAccess::Unavailable
    }
}

/// The CLI's roster of its own running processes.
#[derive(Clone, Debug)]
pub(super) struct ClaudeRoster {
    sessions: Result<PathBuf, HomeProblem>,
    /// Whether an editor-panel session is answering, read from its transcript because such a session writes no
    /// status into the roster. Cheap and cached: an unchanged transcript is one `stat`.
    transcript: TranscriptActivity,
    source: NativeSource,
}

impl ClaudeRoster {
    /// Locate the roster from the environment inherited by the CLI. Opens nothing.
    #[must_use]
    pub(super) fn from_environment() -> Self {
        let config = config_directory(&mut |name| std::env::var_os(name));
        let projects = match &config {
            Ok(directory) => Some(directory.clone()),
            Err(_) => None,
        };
        Self {
            sessions: config.map(|directory| directory.join(SESSIONS_DIRECTORY)),
            transcript: TranscriptActivity::new(projects),
            source: NativeSource::default(),
        }
    }

    #[cfg(test)]
    fn at(sessions: PathBuf) -> Self {
        Self {
            sessions: Ok(sessions),
            transcript: TranscriptActivity::new(None),
            source: NativeSource::default(),
        }
    }

    /// The directory this CLI keeps one record per live process in. Its file set changing is this CLI's own
    /// statement that a session started or ended.
    pub(super) fn sessions_directory(&self) -> Option<PathBuf> {
        match &self.sessions {
            Ok(directory) => Some(directory.clone()),
            Err(_) => None,
        }
    }

    /// The conversations of this CLI whose model is answering right now.
    ///
    /// A roster this driver cannot locate answers with nothing, which is what a machine where the CLI has
    /// never run looks like from here.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the roster directory exists and cannot be listed. A single record that
    /// cannot be read is stepped over instead, because the CLI rewrites those files while this reads them.
    pub(super) fn running(
        &self,
        provider: ProviderId,
    ) -> Result<Vec<NativeSessionId>, ProviderError> {
        Ok(self.activity(provider)?.active)
    }

    /// Every conversation owned by a live CLI process and the subset answering now, from one bounded scan.
    pub(super) fn activity(
        &self,
        provider: ProviderId,
    ) -> Result<NativeProcessActivity, ProviderError> {
        Ok(self.observation(provider)?.activity)
    }

    pub(super) fn watch(
        &self,
        provider: ProviderId,
        scan: &'static crate::roster_scan::RosterScan,
    ) -> Result<Box<dyn runtrol_provider::NativeActivityWatch>, ProviderError> {
        let home = self
            .sessions
            .as_ref()
            .map_or(None, |path| path.parent())
            .ok_or_else(|| ProviderError::Protocol {
                provider,
                doing: "locating native structural observation sources",
                detail: "the provider configuration directory is unavailable".to_owned(),
            })?;
        self.source.watch(
            provider,
            home,
            scan,
            &[
                SESSIONS_DIRECTORY,
                super::store::PROJECTS_DIRECTORY,
                super::store::HISTORY_FILE,
            ],
        )
    }

    pub(super) fn observation(
        &self,
        provider: ProviderId,
    ) -> Result<NativeProcessObservation, ProviderError> {
        let mut records = self.live_records(provider)?;
        records.sort_by(|left, right| {
            left.session_id
                .cmp(&right.session_id)
                .then(left.pid.cmp(&right.pid))
        });
        let mut signature = Sha256::new();
        let mut identities = Vec::new();
        let mut unknown = BTreeSet::new();
        // A conversation continued in a second process is named by two records, so each answer is a set.
        let mut live = BTreeSet::new();
        let mut active = BTreeSet::new();
        let mut processes = Vec::new();
        for entry in records {
            let Ok(native) = NativeSessionId::new(entry.session_id.as_str()) else {
                // An invalid provider identity cannot match a catalogue row or become a claim key. Other
                // valid roster records remain usable, and the provider may replace this record next round.
                continue;
            };
            live.insert(native.clone());
            // A record with a status says whether it is answering; a panel session writes none, so its turn
            // is read from its transcript instead. Only a session with no status pays that read.
            let identity = runtrol_childproc::process_identity(entry.pid);
            let metadata = self.transcript.observation(
                entry.session_id.as_str(),
                identity,
                entry.status.is_none(),
            );
            if metadata.unavailable {
                return Err(ProviderError::Protocol {
                    provider,
                    doing: "reading current native structural metadata",
                    detail: "the original session metadata is temporarily unavailable".into(),
                });
            }
            let model = match entry.status.as_deref() {
                Some(BUSY) => Some(true),
                Some("idle" | "waiting") => Some(false),
                Some(_) => None,
                None => metadata.model,
            };
            signature.update(metadata.revision);
            match model {
                Some(true) => {
                    active.insert(native.clone());
                }
                None => {
                    unknown.insert(native.clone());
                }
                Some(false) => {}
            }
            if let Some(identity) = identity {
                identities.push(identity);
                signature.update(identity.started().to_le_bytes());
            }
            // Length-delimited fields avoid concatenation aliases. No nameplate leaves the driver here.
            for value in [
                Some(entry.session_id.as_str()),
                entry.cwd.as_deref(),
                entry.name.as_deref(),
                entry
                    .name_since
                    .as_deref()
                    .map(serde_json::value::RawValue::get),
                entry.name_source.as_deref(),
                entry
                    .status_updated_at
                    .as_deref()
                    .map(serde_json::value::RawValue::get),
            ] {
                let bytes = value.unwrap_or_default().as_bytes();
                signature.update((bytes.len() as u64).to_le_bytes());
                signature.update(bytes);
            }
            signature.update(entry.pid.to_le_bytes());
            processes.push(NativeProcessBinding {
                pid: entry.pid,
                native,
                cwd: entry.cwd.clone(),
                terminal_access: terminal_access(&entry),
            });
        }
        self.transcript.retain(&live);
        if let Some(stamp) = self.catalogue_stamp(provider)? {
            signature.update(stamp);
        }
        // A second live writer proving activity is stronger than another writer's unavailable proof.
        unknown.retain(|native| !active.contains(native));
        let revision = self
            .source
            .record(provider, signature.finalize().into(), identities)?;
        Ok(NativeProcessObservation {
            activity: NativeProcessActivity {
                live: live.into_iter().collect(),
                active: active.into_iter().collect(),
                processes,
            },
            catalogue_revision: Some(revision),
            unknown_activity: unknown.into_iter().collect(),
        })
    }

    fn catalogue_stamp(&self, provider: ProviderId) -> Result<Option<[u8; 32]>, ProviderError> {
        let Ok(sessions) = &self.sessions else {
            return Ok(None);
        };
        let Some(home) = sessions.parent() else {
            return Ok(None);
        };
        // This is the catalogue's existing fallback name index. Late publication invalidates names
        // without reading a display value here or implying that the model is working.
        crate::native_watch::catalogue_stamp(&home.join(super::store::HISTORY_FILE))
            .map(Some)
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing: "observing provider catalogue metadata",
                detail: error.to_string(),
            })
    }

    /// Whether any still-live process of this CLI owns the selected conversation, regardless of turn state.
    pub(super) fn owns_live(
        &self,
        provider: ProviderId,
        native: &str,
    ) -> Result<bool, ProviderError> {
        Ok(self
            .live_records(provider)?
            .iter()
            .any(|entry| entry.session_id == native))
    }

    fn live_records(&self, provider: ProviderId) -> Result<Vec<Record>, ProviderError> {
        let Ok(sessions) = &self.sessions else {
            return Ok(Vec::new());
        };
        let read_failure = |detail: std::io::Error| ProviderError::Protocol {
            provider,
            doing: "reading which of this CLI's conversations have a model answering",
            detail: detail.to_string(),
        };
        let records = match fs::read_dir(sessions) {
            Ok(entries) => entries,
            // The CLI has not run on this machine yet, or keeps its configuration elsewhere. Neither is a
            // fault to report: both mean nothing of it is running.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(read_failure(error)),
        };
        let mut live = Vec::new();
        for (index, record) in records.enumerate() {
            if index >= MAX_ROSTER_ENTRIES {
                return Err(read_failure(std::io::Error::other(format!(
                    "the process roster exceeds its {MAX_ROSTER_ENTRIES} entry observation bound"
                ))));
            }
            let record = record.map_err(read_failure)?;
            let path = record.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some(RECORD_EXTENSION) {
                continue;
            }
            // A provider can rewrite a live process record in place. Its filename proves a live
            // writer can still be publishing, so an incomplete read is unavailable, never an exit.
            let writer_alive = path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.parse::<u32>().is_ok_and(runtrol_childproc::alive));
            let unreadable = || {
                read_failure(std::io::Error::other(
                    "a live process roster record is not completely readable",
                ))
            };
            let metadata = match record.metadata() {
                Ok(metadata) => metadata,
                Err(_) if writer_alive => return Err(unreadable()),
                Err(_) => continue,
            };
            if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
                if writer_alive {
                    return Err(unreadable());
                }
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(_) if writer_alive => return Err(unreadable()),
                Err(_) => continue,
            };
            let entry = match serde_json::from_str::<Record>(&text) {
                Ok(entry) => entry,
                Err(_) if writer_alive => return Err(unreadable()),
                Err(_) => continue,
            };
            let alive = match entry.proc_start.as_deref() {
                Some(start) => start
                    .parse::<u64>()
                    .is_ok_and(|start| runtrol_childproc::matches_process_start(entry.pid, start)),
                None => runtrol_childproc::alive(entry.pid),
            };
            if !alive {
                continue;
            }
            live.push(entry);
        }
        Ok(live)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    /// A roster directory of this test's own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn claude() -> ProviderId {
        ProviderId::parse("claude").expect("the built-in provider identity parses")
    }

    fn record(pid: u32, session: &str, status: &str) -> String {
        record_with_entrypoint(pid, session, status, "cli")
    }

    fn record_with_entrypoint(pid: u32, session: &str, status: &str, entrypoint: &str) -> String {
        format!(
            "{{\"pid\":{pid},\"sessionId\":\"{session}\",\"cwd\":\"/work\",\"status\":\"{status}\",\"kind\":\"interactive\",\"entrypoint\":\"{entrypoint}\",\"updatedAt\":1}}"
        )
    }

    fn record_with_start(pid: u32, session: &str, status: &str, start: &str) -> String {
        format!(
            "{{\"pid\":{pid},\"procStart\":\"{start}\",\"sessionId\":\"{session}\",\"cwd\":\"/work\",\"status\":\"{status}\",\"kind\":\"interactive\",\"entrypoint\":\"cli\",\"updatedAt\":1}}"
        )
    }

    fn record_with_peer(pid: u32, session: &str, entrypoint: &str) -> String {
        format!(
            "{{\"pid\":{pid},\"sessionId\":\"{session}\",\"cwd\":\"/work\",\"status\":\"idle\",\"kind\":\"bg\",\"entrypoint\":\"{entrypoint}\",\"peerProtocol\":1,\"messagingSocketPath\":\"provider-peer\",\"jobId\":\"job-1\"}}"
        )
    }

    fn roster(files: &[(&str, String)]) -> (Scratch, ClaudeRoster) {
        let serial = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runtrol-claude-roster-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("the scratch roster is created");
        for (name, body) in files {
            fs::write(path.join(name), body).expect("a roster record is written");
        }
        let roster = ClaudeRoster::at(path.clone());
        (Scratch(path), roster)
    }

    #[test]
    fn only_a_busy_conversation_whose_process_is_still_running_is_named() {
        let mine = std::process::id();
        let (_kept, roster) = roster(&[
            (
                "1.json",
                record(mine, "aaaaaaaa-0000-4000-8000-000000000001", "busy"),
            ),
            (
                "2.json",
                record(mine, "aaaaaaaa-0000-4000-8000-000000000002", "idle"),
            ),
            (
                "3.json",
                record(mine, "aaaaaaaa-0000-4000-8000-000000000003", "waiting"),
            ),
            // What a process that has ended left behind, still saying busy. This is the record that kept a
            // conversation turning forever before the process itself was asked about.
            (
                "4.json",
                record(u32::MAX, "aaaaaaaa-0000-4000-8000-000000000004", "busy"),
            ),
        ]);
        let running = roster.running(claude()).expect("the roster is readable");
        let named: Vec<String> = running.iter().map(ToString::to_string).collect();
        assert_eq!(
            named,
            vec!["aaaaaaaa-0000-4000-8000-000000000001".to_owned()]
        );
    }

    #[test]
    fn one_scan_separates_live_process_ownership_from_model_activity() {
        let mine = std::process::id();
        let busy = "aaaaaaaa-0000-4000-8000-000000000011";
        let idle = "aaaaaaaa-0000-4000-8000-000000000012";
        let waiting = "aaaaaaaa-0000-4000-8000-000000000013";
        let (_kept, roster) = roster(&[
            ("11.json", record(mine, busy, "busy")),
            ("12.json", record(mine, idle, "idle")),
            ("13.json", record(mine, waiting, "waiting")),
        ]);

        let activity = roster.activity(claude()).expect("the roster is readable");
        let live: Vec<String> = activity.live.iter().map(ToString::to_string).collect();
        let active: Vec<String> = activity.active.iter().map(ToString::to_string).collect();
        assert_eq!(
            live,
            vec![busy.to_owned(), idle.to_owned(), waiting.to_owned()]
        );
        assert_eq!(active, vec![busy.to_owned()]);
        assert_eq!(activity.processes.len(), 3);
    }

    #[test]
    fn an_editor_panel_session_is_live_and_neither_launch_offers_a_terminal_route() {
        let mine = std::process::id();
        let terminal = "bbbbbbbb-0000-4000-8000-000000000001";
        let panel = "bbbbbbbb-0000-4000-8000-000000000002";
        let (_kept, roster) = roster(&[
            (
                "t.json",
                record_with_entrypoint(mine, terminal, "idle", "cli"),
            ),
            (
                "p.json",
                record_with_entrypoint(mine, panel, "idle", "claude-vscode"),
            ),
        ]);

        let activity = roster.activity(claude()).expect("the roster is readable");
        // Both processes own a live conversation: the panel session is real and belongs in the sidebar.
        let live: Vec<String> = activity.live.iter().map(ToString::to_string).collect();
        assert_eq!(live, vec![terminal.to_owned(), panel.to_owned()]);
        // Neither launch is a route: a console is never joined, so both are observe-only until an official
        // attachment target is published.
        assert!(
            activity.processes.iter().all(|process| matches!(
                &process.terminal_access,
                NativeTerminalAccess::Unavailable
            ))
        );
    }

    #[test]
    fn a_background_job_with_an_official_target_is_attachable_without_a_console() {
        let mine = std::process::id();
        let session = "bbbbbbbb-0000-4000-8000-000000000003";
        let (_kept, roster) = roster(&[(
            "peer.json",
            record_with_peer(mine, session, "claude-vscode"),
        )]);

        let activity = roster.activity(claude()).expect("the roster is readable");
        assert_eq!(activity.processes.len(), 1);
        let process = activity
            .processes
            .first()
            .expect("one process was reported");
        assert_eq!(
            &process.terminal_access,
            &NativeTerminalAccess::Official {
                target: NativeTerminalTarget::new("job-1").expect("a valid opaque target")
            }
        );
    }

    #[test]
    fn an_unbounded_official_target_is_observed_but_never_reaches_argv() {
        let mine = std::process::id();
        let session = "bbbbbbbb-0000-4000-8000-000000000004";
        let oversized = "j".repeat(NativeTerminalTarget::MAX_LEN + 1);
        let record = record_with_peer(mine, session, "claude-vscode").replace("job-1", &oversized);
        let (_kept, roster) = roster(&[("peer.json", record)]);

        let activity = roster.activity(claude()).expect("the roster is readable");
        assert_eq!(activity.processes.len(), 1);
        let process = activity
            .processes
            .first()
            .expect("one process was reported");
        assert!(matches!(
            &process.terminal_access,
            NativeTerminalAccess::Unavailable
        ));
        assert_eq!(
            activity
                .live
                .first()
                .expect("the live conversation was reported")
                .as_str(),
            session
        );
    }

    #[test]
    fn a_stale_record_cannot_alias_a_reused_process_identifier() {
        let mine = std::process::id();
        let session = "aaaaaaaa-0000-4000-8000-000000000099";
        let (_kept, roster) = roster(&[("99.json", record_with_start(mine, session, "busy", "0"))]);

        assert!(
            roster
                .activity(claude())
                .expect("the roster is readable")
                .live
                .is_empty(),
            "the current process did not start at the stale record's zero identity"
        );
    }

    #[test]
    fn one_conversation_taken_over_by_a_second_process_is_named_once() {
        let mine = std::process::id();
        let session = "bbbbbbbb-0000-4000-8000-000000000001";
        let (_kept, roster) = roster(&[
            ("10.json", record(mine, session, "busy")),
            ("11.json", record(mine, session, "busy")),
        ]);
        let running = roster.running(claude()).expect("the roster is readable");
        assert_eq!(
            running.len(),
            1,
            "a conversation is one row however many processes it has had"
        );
    }

    #[test]
    fn an_idle_live_process_still_owns_its_conversation_for_deletion() {
        let mine = std::process::id();
        let session = "bbbbbbbb-0000-4000-8000-000000000099";
        let (_kept, roster) = roster(&[("12.json", record(mine, session, "idle"))]);
        assert!(
            roster
                .owns_live(claude(), session)
                .expect("the roster is readable")
        );
    }

    #[test]
    fn a_record_being_written_and_a_file_that_is_not_one_are_stepped_over() {
        let mine = std::process::id();
        let (_kept, roster) = roster(&[
            (
                "20.json",
                record(mine, "cccccccc-0000-4000-8000-000000000001", "busy"),
            ),
            // Caught mid-write by the CLI.
            ("21.json", "{\"pid\":1,\"sessionI".to_owned()),
            // The CLI keeps its per-process keys in the same directory.
            ("22.key", "not a record at all".to_owned()),
        ]);
        let running = roster
            .running(claude())
            .expect("a half-written record is not an error about the panel");
        assert_eq!(running.len(), 1);
    }

    #[test]
    fn a_machine_where_this_cli_has_never_run_names_nothing() {
        let (kept, _unused) = roster(&[]);
        let roster = ClaudeRoster::at(kept.0.join("never-created"));
        let running = roster
            .running(claude())
            .expect("a missing roster is not a failure");
        assert!(running.is_empty());
    }

    #[test]
    fn an_incomplete_live_writer_record_is_unavailable_instead_of_an_exit() {
        let name = format!("{}.json", std::process::id());
        let (_kept, roster) = roster(&[(&name, "{\"pid\":".to_owned())]);
        assert!(roster.observation(claude()).is_err());
    }

    #[test]
    fn a_late_fallback_title_index_invalidates_catalogue_without_changing_activity() {
        let (kept, _unused) = roster(&[]);
        let sessions = kept.0.join(SESSIONS_DIRECTORY);
        fs::create_dir(&sessions).expect("isolated provider roster");
        fs::write(
            sessions.join("owner.json"),
            record(
                std::process::id(),
                "dddddddd-0000-4000-8000-000000000001",
                "idle",
            ),
        )
        .expect("provider owns an idle conversation");
        let roster = ClaudeRoster::at(sessions);
        let first = roster
            .observation(claude())
            .expect("no published title index");
        fs::write(kept.0.join(super::super::store::HISTORY_FILE), b"{}\n")
            .expect("provider creates its metadata index");
        let second = roster.observation(claude()).expect("index appears");
        assert_ne!(first.catalogue_revision, second.catalogue_revision);
        assert_eq!(first.activity, second.activity);
        assert_eq!(
            second.catalogue_revision,
            roster
                .observation(claude())
                .expect("unchanged index")
                .catalogue_revision
        );
    }
}
