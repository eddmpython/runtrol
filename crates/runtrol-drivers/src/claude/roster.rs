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

use runtrol_provider::{NativeSessionId, ProviderError, ProviderId};
use serde::Deserialize;

use crate::claude::home::{HomeProblem, config_directory};

/// Where the CLI writes one file per running process of itself.
const SESSIONS_DIRECTORY: &str = "sessions";

/// The extension of a roster record. The directory holds the CLI's per-process keys as well, which are not
/// records and are not read.
const RECORD_EXTENSION: &str = "json";

/// The largest a roster record may be. Measured on 2.1.250: 607 bytes at the largest, and a file far past that
/// is not the small record this reads, so it is stepped over rather than parsed.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// What the CLI calls a process of itself while a model is answering in it.
const BUSY: &str = "busy";

/// The CLI's own record of one running process of itself. Only the fields this question needs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The operating system process that wrote this record.
    pid: u32,
    /// The conversation that process is in, named the way the stored conversation is named.
    session_id: String,
    /// What the process is doing. Absent in a record written before the CLI had the field.
    #[serde(default)]
    status: Option<String>,
}

/// The CLI's roster of its own running processes.
#[derive(Clone, Debug)]
pub(super) struct ClaudeRoster {
    sessions: Result<PathBuf, HomeProblem>,
}

impl ClaudeRoster {
    /// Locate the roster from the environment inherited by the CLI. Opens nothing.
    #[must_use]
    pub(super) fn from_environment() -> Self {
        Self {
            sessions: config_directory(&mut |name| std::env::var_os(name))
                .map(|directory| directory.join(SESSIONS_DIRECTORY)),
        }
    }

    #[cfg(test)]
    fn at(sessions: PathBuf) -> Self {
        Self {
            sessions: Ok(sessions),
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
        // A conversation continued in a second process is named by two records, so the answer is a set.
        let mut running = BTreeSet::new();
        for record in records {
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
                continue;
            };
            let Ok(entry) = serde_json::from_str::<Record>(&text) else {
                // The CLI owns this file and rewrites it whenever a status changes, so a read can land on a
                // half-written one. Stepping over it costs one round: the next poll reads the finished file.
                // Reporting it would turn the CLI's own write into an error about the panel.
                continue;
            };
            if entry.status.as_deref() != Some(BUSY) {
                continue;
            }
            if !runtrol_childproc::alive(entry.pid) {
                continue;
            }
            let Ok(native) = NativeSessionId::new(entry.session_id.as_str()) else {
                continue;
            };
            running.insert(native);
        }
        Ok(running.into_iter().collect())
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
        format!(
            "{{\"pid\":{pid},\"sessionId\":\"{session}\",\"cwd\":\"/work\",\"status\":\"{status}\",\"updatedAt\":1}}"
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
