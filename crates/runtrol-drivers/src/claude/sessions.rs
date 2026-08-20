//! The sessions this CLI is running, read from its own command.
//!
//! # What this answers, and what it does not
//!
//! `claude agents --json` prints the sessions running now, and `--all` adds the background ones it kept. It does
//! not print the conversations stored on disk, and its own help says so in one word: `Print active sessions`.
//! Reading the field names and concluding otherwise is a mistake this repository has now made twice, in both
//! directions, so the shape of the answer is written down rather than inferred: this surface is a live roster.
//!
//! It is still worth having. Every claude a person started outside runtrol was invisible while it ran, including
//! the ones waiting on that person. A roster answers exactly that, and it is an official machine-readable command
//! rather than a guess at a private file.
//!
//! Coverage is therefore never complete, and the reason says what is missing rather than how many were dropped.
//! An operator told "these are the ones running" can act on it. An operator told "these are all your
//! conversations" would stop looking for the rest.
//!
//! # What is read
//!
//! Identity, working directory, the CLI's own display name, and the start time. Nothing here carries a
//! conversation: the command reports processes, not transcripts, and `deny_unknown_fields` is left off so a new
//! field is ignored rather than fatal.

use std::time::Duration;

use runtrol_childproc::{Containment, Program, capture};
use runtrol_provider::{
    AbsPath, MAX_NATIVE_SESSION_ITEMS, NativeCatalogueCoverage, NativeCatalogueSource,
    NativeResumeCapability, NativeSessionCatalogue, NativeSessionEntry, NativeSessionId,
    NativeSessionQuery, ProviderError, ProviderId,
};
use serde::Deserialize;

use crate::catalogue::{bounded, under};

/// How long the roster command may take.
///
/// It reads a directory of small registry files it already maintains. Generous against that, and short enough
/// that a hung child cannot make the conversation list feel broken.
const LISTING_DEADLINE: Duration = Duration::from_secs(15);

/// One entry of the roster, reduced to what a catalogue entry needs.
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    /// The CLI's own display name for the session, which is what it shows in its own prompt box.
    ///
    /// Not a conversation summary and not offered as one. It is the label the CLI chose, relayed unchanged.
    name: Option<String>,
    /// Milliseconds since the epoch, which is this CLI's own representation.
    #[serde(rename = "startedAt")]
    started_at: Option<u64>,
}

/// Ask the CLI which of its sessions are running.
///
/// # Errors
///
/// [`ProviderError::Protocol`] when the command answers in a shape this driver cannot read. The command exists
/// and answered, so reporting it as an absent surface would tell an operator to stop looking for sessions that
/// are there.
pub(super) async fn list(
    provider: ProviderId,
    program: &Program,
    resumable: bool,
    query: &NativeSessionQuery,
    contained_by: &Containment,
) -> Result<NativeSessionCatalogue, ProviderError> {
    // Measured on 2.1.234: `--cwd` narrows interactive sessions as well as background ones, although the help
    // text mentions only the latter. Sent because a narrower answer is cheaper, and the entries are filtered
    // again below because a flag whose documented meaning is narrower than its behaviour may not be leaned on.
    let mut arguments = vec!["agents".to_owned(), "--json".to_owned(), "--all".to_owned()];
    // Only when a folder was asked for. `--cwd` is a filter here, not a required argument
    // (measured 2026-08-20: the help calls it "Show only background sessions started under
    // <path>", and omitting it returned eight rows across four projects), so a machine-wide query
    // leaves it off. What stays true either way is what this command answers: the sessions this
    // CLI is running, never the conversations it has stored. The coverage below says so.
    if let Some(root) = query.root.as_ref() {
        arguments.push("--cwd".to_owned());
        arguments.push(root.as_str().to_owned());
    }
    let output = capture(program, &arguments, LISTING_DEADLINE, contained_by)
        .await
        .map_err(|error| ProviderError::Protocol {
            provider,
            doing: "listing the sessions this CLI is running",
            detail: error.to_string(),
        })?;
    if output.truncated {
        return Err(ProviderError::Protocol {
            provider,
            doing: "listing the sessions this CLI is running",
            detail: "the answer was longer than the bounded read".to_owned(),
        });
    }
    let records: Vec<Record> =
        serde_json::from_str(output.text().trim()).map_err(|error| ProviderError::Protocol {
            provider,
            doing: "reading the session roster this CLI printed",
            detail: error.to_string(),
        })?;
    Ok(page(
        records,
        query.root.as_ref().map(AbsPath::as_str),
        query.limit,
        resumable,
    ))
}

/// Turn roster entries into one bounded page.
///
/// Filtering by the requested root here is a convenience, not the security boundary: Runtime canonicalises and
/// re-checks every entry against the caller's approved roots before anything is shown.
fn page(
    records: Vec<Record>,
    root: Option<&str>,
    limit: u16,
    resumable: bool,
) -> NativeSessionCatalogue {
    let capacity = usize::from(limit).min(MAX_NATIVE_SESSION_ITEMS);
    let mut sessions: Vec<NativeSessionEntry> = Vec::new();
    for record in records {
        if sessions.len() >= capacity {
            break;
        }
        let identity = record.session_id.as_deref().map(NativeSessionId::new);
        let Some(Ok(native)) = identity else {
            // One unusable identifier costs one row. The others are still real sessions, and the coverage below
            // already tells the operator this page is not the whole picture.
            continue;
        };
        let Some(cwd) = record.cwd.filter(|path| !path.is_empty()) else {
            continue;
        };
        // A folder narrows the roster; no folder means every session this CLI is running.
        if root.is_some_and(|root| !under(&cwd, root)) {
            continue;
        }
        let title = record
            .name
            .map(|text| bounded(text.trim()))
            .filter(|text| !text.is_empty());
        sessions.push(NativeSessionEntry {
            native,
            cwd: cwd.into(),
            // The roster reports one directory per session. Claiming more would be inventing authority.
            additional_directories: Vec::new(),
            title: title.map(Into::into),
            // The CLI's own representation, relayed unchanged. The protocol asks for what the provider said and
            // not for a house format, so the digits travel and the surface reads them.
            updated_at: record.started_at.map(|at| at.to_string().into()),
            resume: if resumable {
                NativeResumeCapability::Available
            } else {
                NativeResumeCapability::Unknown
            },
        });
    }
    NativeSessionCatalogue {
        // Never complete, whatever came back. This command answers "which sessions are running", and the
        // conversations this CLI has stored are not in it. Saying so is what keeps an operator looking.
        coverage: NativeCatalogueCoverage::Partial {
            source: NativeCatalogueSource::OfficialCli,
            why: "this CLI lists the sessions it is running, not the conversations it has stored"
                .into(),
        },
        sessions,
        // The roster is one answer with no paging surface, so there is nothing to continue from.
        next_cursor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roster shape, copied from `claude agents --json --all` on 2.1.234 with the paths shortened.
    ///
    /// Copied rather than composed. Both spellings of the drive letter are in the real answer for one folder, and
    /// a hand-written fixture would have agreed with whichever the author happened to type.
    const REAL_SHAPE: &str = r#"[
      {"pid":9616,"cwd":"C:\\work\\alpha","kind":"interactive",
       "startedAt":1787006687501,"sessionId":"98da7684-d0cf-4962-b4af-9b35dd42972b","name":"alpha-87"},
      {"pid":16396,"cwd":"c:\\work\\alpha","kind":"interactive",
       "startedAt":1787006699128,"sessionId":"01a011e6-414e-7601-9f70-2f66980e2acd","name":"alpha-54"},
      {"pid":4136,"cwd":"C:\\work\\beta","kind":"background","state":"failed",
       "startedAt":1787013318134,"sessionId":"fc2e97a4-1030-43fe-ae32-e78e79351ce1","name":"beta-d9"}
    ]"#;

    fn decoded(root: Option<&str>, resumable: bool) -> NativeSessionCatalogue {
        let records: Vec<Record> =
            serde_json::from_str(REAL_SHAPE).expect("the real shape decodes");
        page(records, root, 50, resumable)
    }

    #[test]
    fn a_session_started_outside_runtrol_becomes_a_row() {
        // The whole reason this module exists. Every claude a person started in their own terminal was invisible
        // while it ran, including the ones waiting on that person.
        let catalogue = decoded(Some("C:/work/alpha"), true);
        assert_eq!(catalogue.sessions.len(), 2);
        let first = catalogue
            .sessions
            .first()
            .expect("the fixture decoded two entries");
        assert_eq!(
            first.native.as_str(),
            "98da7684-d0cf-4962-b4af-9b35dd42972b"
        );
        assert_eq!(first.title.as_deref(), Some("alpha-87"));
        assert_eq!(first.updated_at.as_deref(), Some("1787006687501"));
    }

    #[test]
    fn the_drive_letter_this_cli_happened_to_print_does_not_split_a_folder() {
        // Measured: one folder appears as both `C:\` and `c:\` in the same answer. Comparing the text would show
        // an operator half of their running sessions.
        assert_eq!(decoded(Some("C:/work/alpha"), true).sessions.len(), 2);
        assert_eq!(decoded(Some("c:/work/alpha"), true).sessions.len(), 2);
    }

    #[test]
    fn a_roster_is_never_reported_as_the_whole_history() {
        // The mistake this module is written against. The command prints running sessions; the conversations on
        // disk are not in it. An operator told this page is complete stops looking for the rest.
        for root in ["C:/work/alpha", "C:/work/beta", "C:/work/nothing-here"] {
            assert!(
                matches!(
                    decoded(Some(root), true).coverage,
                    NativeCatalogueCoverage::Partial {
                        source: NativeCatalogueSource::OfficialCli,
                        ..
                    }
                ),
                "a roster is never complete, including when it is empty"
            );
        }
    }

    #[test]
    fn resume_is_reported_from_the_flag_the_cli_confirmed() {
        // Runtime only offers to open a row whose resume is available, so claiming it without the flag would put
        // a row on screen that fails on click. Claiming it is unknown when the flag was confirmed would hide
        // every running session behind an unopenable row.
        assert_eq!(
            decoded(Some("C:/work/alpha"), true)
                .sessions
                .first()
                .expect("an entry")
                .resume,
            NativeResumeCapability::Available
        );
        assert_eq!(
            decoded(Some("C:/work/alpha"), false)
                .sessions
                .first()
                .expect("an entry")
                .resume,
            NativeResumeCapability::Unknown
        );
    }

    #[test]
    fn a_session_in_another_folder_is_not_offered() {
        assert!(decoded(Some("C:/work/gamma"), true).sessions.is_empty());
    }

    #[test]
    fn nothing_that_carries_a_conversation_survives_the_decoder() {
        // The roster reports processes rather than transcripts, and this asserts the decoder keeps it that way
        // if the CLI ever starts including more.
        let records: Vec<Record> = serde_json::from_str(
            r#"[{"sessionId":"s","cwd":"C:\\work\\alpha","name":"alpha-1",
                 "lastPrompt":"A_CONVERSATION_BODY","transcript":"A_TRANSCRIPT_PATH"}]"#,
        )
        .expect("decodes");
        let catalogue = page(records, Some("C:/work/alpha"), 50, true);
        let rendered = format!("{catalogue:?}");
        for forbidden in [
            "A_CONVERSATION_BODY",
            "A_TRANSCRIPT_PATH",
            "lastPrompt",
            "transcript",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "{forbidden} reached a catalogue entry"
            );
        }
    }
}
