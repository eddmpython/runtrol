//! Reading a CLI's own conversation list from its own command.
//!
//! Some CLIs whose protocol has no session enumeration method print one on their command line. This module asks
//! that command and turns the answer into catalogue entries.
//!
//! # The one rule this module exists to keep
//!
//! A coding CLI's history record contains the conversation. Measured on cline 3.0.52, one record carries the
//! operator's prompt in full, a path to the stored messages, token counts and costs. **Four fields are read and
//! the rest is never touched**: identity, working directory, title, timestamp. Everything else stays in the child's
//! stdout and is dropped with it.
//!
//! That is not tidiness. runtrol keeping any part of a conversation is the one thing this product refuses, and a
//! list is exactly where the refusal erodes: somebody adds a preview so the row reads better, then a last message
//! so sorting works. So the decoder names the four fields it wants and `deny_unknown_fields` is deliberately not
//! used, because the point is to ignore the rest rather than to fail on it.

use std::time::Duration;

use runtrol_childproc::{Containment, Program, capture};
use runtrol_provider::{
    MAX_NATIVE_SESSION_ITEMS, MAX_NATIVE_TITLE_BYTES, NativeCatalogueCoverage,
    NativeCatalogueSource, NativeResumeCapability, NativeSessionCatalogue, NativeSessionEntry,
    NativeSessionId, NativeSessionQuery, ProviderError, ProviderId,
};
use serde::Deserialize;

/// How long the CLI's own listing may take.
///
/// Measured against cline 3.0.52 with a real history: it answers in under a second because it reads a file it
/// already maintains. Generous against that, and short enough that a hung child cannot make the conversation list
/// feel broken.
const LISTING_DEADLINE: Duration = Duration::from_secs(15);

/// One record from the CLI's listing, reduced to what a catalogue entry needs.
///
/// Absent `deny_unknown_fields` on purpose. The record holds conversation content and this decoder's job is to
/// leave it there, not to refuse a CLI for having it.
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    /// Only the title is taken out of the metadata object. Everything beside it there is the conversation, its
    /// cost, or a path into stored messages.
    metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Metadata {
    title: Option<String>,
}

/// Ask the CLI for the conversations it owns.
///
/// # Errors
///
/// [`ProviderError::Protocol`] when the command answers in a shape this driver cannot read. That is a vendor-facing
/// fact rather than an absent surface: the command exists and answered, so reporting it as unsupported would tell
/// an operator to stop looking for conversations that are there.
pub(super) async fn list(
    provider: ProviderId,
    program: &Program,
    argv: &[Box<str>],
    limit_flag: Option<&str>,
    query: &NativeSessionQuery,
    contained_by: &Containment,
) -> Result<NativeSessionCatalogue, ProviderError> {
    let capacity = usize::from(query.limit).min(MAX_NATIVE_SESSION_ITEMS);
    let mut arguments: Vec<String> = argv.iter().map(ToString::to_string).collect();
    // One more than will be kept. A CLI that paginates gives a short answer and a truncated answer the same
    // shape, and asking for the extra record is the only thing that tells them apart.
    let asked = limit_flag.map(|flag| {
        let asked = capacity.saturating_add(1);
        arguments.push(flag.to_owned());
        arguments.push(asked.to_string());
        asked
    });
    let output = capture(program, &arguments, LISTING_DEADLINE, contained_by)
        .await
        .map_err(|error| ProviderError::Protocol {
            provider,
            doing: "listing the conversations this CLI owns",
            detail: error.to_string(),
        })?;
    if output.truncated {
        return Err(ProviderError::Protocol {
            provider,
            doing: "listing the conversations this CLI owns",
            detail: "the answer was longer than the bounded read".to_owned(),
        });
    }
    let records: Vec<Record> =
        serde_json::from_str(output.text().trim()).map_err(|error| ProviderError::Protocol {
            provider,
            doing: "reading the conversation list this CLI printed",
            detail: error.to_string(),
        })?;
    Ok(page(records, query.root.as_str(), query.limit, asked))
}

/// Turn records into one bounded page.
///
/// Filtering by the requested root here is a convenience, not the security boundary: Runtime canonicalises and
/// re-checks every entry against the caller's approved roots before anything is shown. Doing it early only keeps
/// the page from being filled with rows that would be discarded.
///
/// `asked` is how many records the CLI was told to print, when the driver could tell it. It is what decides
/// completeness. Filtering by root is deliberately not counted as an omission, because a conversation belonging
/// to another folder is not missing from this folder's answer. What would make the answer wrong is the CLI having
/// stopped early, and that is knowable only by receiving as many records as were requested.
fn page(
    records: Vec<Record>,
    root: &str,
    limit: u16,
    asked: Option<usize>,
) -> NativeSessionCatalogue {
    let capacity = usize::from(limit).min(MAX_NATIVE_SESSION_ITEMS);
    let cut_short = match asked {
        Some(asked) => records.len() >= asked,
        // The CLI paginates and this build does not know the flag that bounds it, so a full page and a whole
        // history are indistinguishable. Saying so is the only honest answer.
        None => true,
    };
    let mut sessions: Vec<NativeSessionEntry> = Vec::new();
    let mut dropped = 0_usize;
    for record in records {
        if sessions.len() >= capacity {
            dropped += 1;
            continue;
        }
        let identity = record.session_id.as_deref().map(NativeSessionId::new);
        let Some(Ok(native)) = identity else {
            // Skipped rather than failing the page. An identifier this build cannot accept costs one row; the
            // other conversations in the answer are still real, and the coverage below reports the omission
            // instead of letting the page claim it holds everything.
            dropped += 1;
            continue;
        };
        let Some(cwd) = record.cwd.filter(|path| !path.is_empty()) else {
            dropped += 1;
            continue;
        };
        if !under(&cwd, root) {
            continue;
        }
        let title = record
            .metadata
            .and_then(|metadata| metadata.title)
            .map(|text| bounded(text.trim()))
            .filter(|text| !text.is_empty());
        sessions.push(NativeSessionEntry {
            native,
            cwd: cwd.into(),
            // The command reports one directory per conversation. Claiming more would be inventing authority.
            additional_directories: Vec::new(),
            title: title.map(Into::into),
            updated_at: record.updated_at.map(Into::into),
            // Resume support is a protocol fact, not a listing fact. The listing says a conversation exists; only
            // the agent handshake says whether it can be reopened, so this stays unknown rather than optimistic.
            resume: NativeResumeCapability::Unknown,
        });
    }
    let coverage = match (cut_short, dropped) {
        (false, 0) => NativeCatalogueCoverage::Complete {
            source: NativeCatalogueSource::OfficialCli,
        },
        (true, 0) => NativeCatalogueCoverage::Partial {
            source: NativeCatalogueSource::OfficialCli,
            why:
                "this CLI printed a full page, so it holds conversations Runtime has not been shown"
                    .into(),
        },
        (false, _) => NativeCatalogueCoverage::Partial {
            source: NativeCatalogueSource::OfficialCli,
            why: "Runtime read this CLI's whole listing and could not use every record in it"
                .into(),
        },
        (true, _) => NativeCatalogueCoverage::Partial {
            source: NativeCatalogueSource::OfficialCli,
            why: "this CLI printed a full page and Runtime could not use every record in it".into(),
        },
    };
    NativeSessionCatalogue {
        coverage,
        sessions,
        // The listing command paginates by page number rather than by an opaque cursor, and a page number is not a
        // cursor Runtime can carry safely across a reconnect. One honest page beats a cursor that means something
        // different the next time the history changes.
        next_cursor: None,
    }
}

/// Whether one reported directory sits inside the requested root.
///
/// Compares on separators rather than on raw text so that a sibling whose name merely starts with the root's name
/// is not mistaken for a child. Case folding is left to Runtime's canonicalisation, which owns the platform rules.
fn under(cwd: &str, root: &str) -> bool {
    let normalise = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_owned();
    let cwd = normalise(cwd);
    let root = normalise(root);
    if root.is_empty() {
        return true;
    }
    cwd.eq_ignore_ascii_case(&root)
        || cwd.len() > root.len()
            && cwd[..root.len()].eq_ignore_ascii_case(&root)
            && cwd.as_bytes().get(root.len()) == Some(&b'/')
}

fn bounded(text: &str) -> String {
    if text.len() <= MAX_NATIVE_TITLE_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_NATIVE_TITLE_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape cline 3.0.52 prints, trimmed to two records and with the conversation-bearing fields kept
    /// so that the isolation test below has something to catch.
    const REAL_SHAPE: &str = r#"[
      {"sessionId":"1786371910187_6lyxh","source":"cli","pid":3268,"status":"failed",
       "provider":"cline","model":"openai/gpt-5.6-sol","cwd":"/work/alpha",
       "prompt":"CONVERSATION_BODY_THAT_MUST_NOT_BE_READ",
       "metadata":{"title":"a title the CLI made","totalCost":0.28,
                   "usage":{"inputTokens":655015},
                   "checkpoint":{"latest":{"ref":"64a89d6"}}},
       "messagesPath":"/home/me/.cline/data/sessions/1786371910187_6lyxh.messages.json",
       "updatedAt":"2026-08-17T16:04:38.309Z"},
      {"sessionId":"1786371910188_zzzzz","cwd":"/work/alpha/nested",
       "prompt":"ANOTHER_CONVERSATION_BODY","metadata":{"title":"second"},
       "updatedAt":"2026-08-17T17:00:00.000Z"}
    ]"#;

    /// The fixture, read as a driver that asked for one more record than it would keep. Two records come back,
    /// so any request for two or more proves the CLI had nothing further.
    fn decoded(root: &str, limit: u16) -> NativeSessionCatalogue {
        let records: Vec<Record> =
            serde_json::from_str(REAL_SHAPE).expect("the real shape decodes");
        let asked = usize::from(limit).saturating_add(1);
        page(records, root, limit, Some(asked))
    }

    #[test]
    fn the_four_fields_a_catalogue_needs_come_through() {
        let catalogue = decoded("/work/alpha", 50);
        assert_eq!(catalogue.sessions.len(), 2);
        let first = catalogue
            .sessions
            .first()
            .expect("the fixture decoded two entries");
        assert_eq!(first.native.as_str(), "1786371910187_6lyxh");
        assert_eq!(&*first.cwd, "/work/alpha");
        assert_eq!(first.title.as_deref(), Some("a title the CLI made"));
        assert_eq!(
            first.updated_at.as_deref(),
            Some("2026-08-17T16:04:38.309Z")
        );
    }

    #[test]
    fn nothing_that_carries_a_conversation_survives_the_decoder() {
        // The rule this module exists for. The record holds the operator's prompt in full, a path to the stored
        // messages, token counts and costs, and a list is exactly where that starts leaking in: somebody adds a
        // preview so the row reads better. Asserted on the whole decoded page rather than field by field, so a
        // field added later is covered the day it is written.
        let catalogue = decoded("/work/alpha", 50);
        let rendered = format!("{catalogue:?}");
        for forbidden in [
            "CONVERSATION_BODY_THAT_MUST_NOT_BE_READ",
            "ANOTHER_CONVERSATION_BODY",
            "messagesPath",
            ".messages.json",
            "totalCost",
            "inputTokens",
            "checkpoint",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "{forbidden} reached a catalogue entry"
            );
        }
    }

    #[test]
    fn a_conversation_outside_the_requested_root_is_not_offered() {
        let catalogue = decoded("/work/beta", 50);
        assert!(catalogue.sessions.is_empty());
    }

    #[test]
    fn a_sibling_whose_name_starts_with_the_root_is_not_inside_it() {
        // `/work/alpha-other` is not under `/work/alpha`, and a plain prefix comparison would say it is.
        assert!(under("/work/alpha/nested", "/work/alpha"));
        assert!(under("/work/alpha", "/work/alpha"));
        assert!(!under("/work/alpha-other", "/work/alpha"));
        assert!(under("C:\\work\\alpha\\deep", "C:/work/alpha"));
    }

    #[test]
    fn a_page_that_had_to_drop_something_says_so_instead_of_claiming_completeness() {
        let complete = decoded("/work/alpha", 50);
        assert!(matches!(
            complete.coverage,
            NativeCatalogueCoverage::Complete {
                source: NativeCatalogueSource::OfficialCli
            }
        ));
        let clipped = decoded("/work/alpha", 1);
        assert_eq!(clipped.sessions.len(), 1);
        assert!(matches!(
            clipped.coverage,
            NativeCatalogueCoverage::Partial {
                source: NativeCatalogueSource::OfficialCli,
                ..
            }
        ));
    }

    #[test]
    fn a_full_page_is_never_reported_as_the_whole_history() {
        // The defect this rule exists for, measured on cline 3.0.55. The command's own default is fifty and the
        // manifest asked for no bound, so a history longer than that printed a page holding none of the requested
        // root's conversations. Every record in it was usable and none belonged here, which made `dropped` zero
        // and the answer "no conversations, and that is all of them". Filtering by root is not an omission; a page
        // that ended because the CLI stopped counting is.
        let records: Vec<Record> = serde_json::from_str(
            r#"[{"sessionId":"a","cwd":"/elsewhere"},{"sessionId":"b","cwd":"/elsewhere"}]"#,
        )
        .expect("decodes");
        let catalogue = page(records, "/work/alpha", 1, Some(2));
        assert!(
            catalogue.sessions.is_empty(),
            "nothing in the page belongs to this root"
        );
        assert!(
            matches!(catalogue.coverage, NativeCatalogueCoverage::Partial { .. }),
            "an empty answer off a full page is not a complete answer"
        );
    }

    #[test]
    fn a_short_page_proves_the_cli_had_nothing_further() {
        // The other half. Asking for one more than will be kept is what makes completeness knowable at all, so a
        // driver that received fewer records than it asked for has seen the CLI's whole listing.
        let records: Vec<Record> =
            serde_json::from_str(r#"[{"sessionId":"a","cwd":"/work/alpha"}]"#).expect("decodes");
        let catalogue = page(records, "/work/alpha", 50, Some(51));
        assert!(matches!(
            catalogue.coverage,
            NativeCatalogueCoverage::Complete { .. }
        ));
    }

    #[test]
    fn a_cli_whose_bound_is_unknown_never_claims_completeness() {
        // Without the flag that bounds the listing, a whole history and a first page arrive in the same shape.
        // Guessing between them is how an operator comes to trust a list that is missing conversations.
        let records: Vec<Record> =
            serde_json::from_str(r#"[{"sessionId":"a","cwd":"/work/alpha"}]"#).expect("decodes");
        let catalogue = page(records, "/work/alpha", 50, None);
        assert!(matches!(
            catalogue.coverage,
            NativeCatalogueCoverage::Partial { .. }
        ));
    }

    #[test]
    fn resume_stays_unknown_because_a_listing_cannot_know_it() {
        // The listing says a conversation exists. Only the agent handshake says whether it can be reopened, and
        // claiming availability here would offer a row that fails on click.
        for entry in decoded("/work/alpha", 50).sessions {
            assert_eq!(entry.resume, NativeResumeCapability::Unknown);
        }
    }

    #[test]
    fn a_record_without_an_identity_is_skipped_rather_than_invented() {
        let records: Vec<Record> =
            serde_json::from_str(r#"[{"cwd":"/work/alpha"},{"sessionId":"","cwd":"/work/alpha"}]"#)
                .expect("decodes");
        let catalogue = page(records, "/work/alpha", 50, Some(51));
        assert!(catalogue.sessions.is_empty());
        assert!(matches!(
            catalogue.coverage,
            NativeCatalogueCoverage::Partial { .. }
        ));
    }
}
