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
    NativeProcessActivity, NativeProcessBinding, NativeSessionId, NativeTerminalAccess,
    NativeTerminalTarget, ProviderError, ProviderId,
};
use serde::Deserialize;

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
    /// `interactive` for the CLI's own terminal interface; other values are piped or SDK children.
    #[serde(default)]
    kind: Option<String>,
    /// How the process was launched, in the CLI's own words: `cli` is a real terminal it owns; `claude-vscode`,
    /// `vscode`, `sdk`, `print` and `mcp` are piped children of another program with no console of their own.
    /// A record from a CLI too old to write this is `None`, and the launch kind falls back to `kind`.
    #[serde(default)]
    entrypoint: Option<String>,
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
}

/// Whether a roster record names a process with a console another window can join and type into.
///
/// Only the CLI's own terminal launch (`entrypoint` = `cli`) owns a real console. A `claude-vscode`, `vscode`,
/// `sdk`, `print` or `mcp` process is a piped child of another program: it reports `kind` = `interactive` from
/// its own point of view, but has no console, so mirroring it attaches a helper to nothing and the mirror dies
/// the instant it is made (operator, 2026-08-30: the editor's Claude panel session flickered in and out of the
/// sidebar). The positive test on `cli` fails safe: a launch kind this build has not seen is treated as
/// non-joinable and shown as running elsewhere, never mirrored. A record from a CLI too old to write an
/// entrypoint falls back to the older `kind` signal so its terminal sessions still mirror.
fn has_a_console_to_join(entrypoint: Option<&str>, kind: Option<&str>) -> bool {
    match entrypoint {
        Some(launch) => launch == "cli" && kind == Some("interactive"),
        None => kind == Some("interactive"),
    }
}

/// The strongest honest route into a live terminal session.
///
/// Official attachment wins over console mirroring because it preserves the provider's own byte stream and works
/// for background jobs that own no interactive operating-system console. A record without the provider's complete
/// attachment target falls back to the measured console rule used by older CLI versions.
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
    } else if has_a_console_to_join(record.entrypoint.as_deref(), record.kind.as_deref()) {
        NativeTerminalAccess::Console
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
        }
    }

    #[cfg(test)]
    fn at(sessions: PathBuf) -> Self {
        Self {
            sessions: Ok(sessions),
            transcript: TranscriptActivity::new(None),
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
        let records = self.live_records(provider)?;
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
            let is_answering = match entry.status.as_deref() {
                Some(status) => status == BUSY,
                None => self.transcript.answering(entry.session_id.as_str()),
            };
            if is_answering {
                active.insert(native.clone());
            }
            processes.push(NativeProcessBinding {
                pid: entry.pid,
                native,
                cwd: entry.cwd.clone(),
                terminal_access: terminal_access(&entry),
            });
        }
        Ok(NativeProcessActivity {
            live: live.into_iter().collect(),
            active: active.into_iter().collect(),
            processes,
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
            let Ok(metadata) = record.metadata() else {
                // Gone between being listed and being asked about: a process that ended while this ran is not
                // one this answer is about.
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                // The provider replaces and rewrites these small records. A path may disappear or become
                // temporarily unreadable between metadata and read; the next 250 ms observation retries it.
                // Treating the entire roster as absent would hide every other live conversation.
                continue;
            };
            let Ok(entry) = serde_json::from_str::<Record>(&text) else {
                // The CLI owns this file and rewrites it whenever a status changes, so a read can land on a
                // half-written one. Stepping over it costs one round: the next poll reads the finished file.
                // Reporting it would turn the CLI's own write into an error about the panel.
                continue;
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
    fn an_editor_panel_session_is_live_but_has_no_console_to_join() {
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
        // Only the terminal launch can be joined and mirrored; the piped panel child cannot.
        let joinable: Vec<&str> = activity
            .processes
            .iter()
            .filter(|process| matches!(&process.terminal_access, NativeTerminalAccess::Console))
            .map(|process| process.native.as_str())
            .collect();
        assert_eq!(joinable, vec![terminal]);
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
    fn a_terminal_session_from_a_cli_too_old_to_write_an_entrypoint_still_mirrors() {
        let mine = std::process::id();
        let session = "cccccccc-0000-4000-8000-000000000021";
        // No kind and no entrypoint: the older builder wrote neither. The kind fallback keeps it joinable.
        let legacy = format!(
            "{{\"pid\":{mine},\"sessionId\":\"{session}\",\"cwd\":\"/work\",\"status\":\"idle\",\"kind\":\"interactive\",\"updatedAt\":1}}"
        );
        let (_kept, roster) = roster(&[("legacy.json", legacy)]);
        let activity = roster.activity(claude()).expect("the roster is readable");
        assert_eq!(activity.processes.len(), 1);
        assert!(
            activity
                .processes
                .iter()
                .all(|process| matches!(&process.terminal_access, NativeTerminalAccess::Console))
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
}
