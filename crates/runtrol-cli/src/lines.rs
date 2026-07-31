//! Turning an answer into the lines a person reads.
//!
//! One thing per line, so that a listing is something another program can read as well as a person. A surface only a
//! person can read is a surface nothing can build on, and this is the surface every script somebody writes around
//! runtrol will be written against.
//!
//! An event is printed as the provider wrote it. Laying it out differently would be this surface reading a
//! conversation, which is the one thing runtrol does not do.

use runtrol_ipc::wire::Response;

/// What a listing prints where a conversation's own name would go, before the provider has said one.
///
/// A placeholder rather than an empty column, so that the number of fields on a line is the same whatever state a
/// session is in. Anything reading this surface can then split on whitespace without special cases.
pub const NOT_NAMED_YET: &str = "-";

/// Turn an answer into the lines a person reads.
///
/// One line each, so a listing is something another program can read too. A surface only a person can read is a surface
/// nothing can build on.
#[must_use]
pub fn render(response: &Response) -> Vec<String> {
    match response {
        Response::Welcome { providers, .. } => providers
            .iter()
            .map(|one| {
                // Shown rather than hidden. An operator with a perfectly good manifest for a kind this build has no
                // driver for should see it marked, not wonder where their provider went.
                let mark = match (one.usable, &one.why_not) {
                    (true, _) => String::new(),
                    (false, Some(why)) => format!("  (unavailable: {why})"),
                    (false, None) => "  (unavailable)".to_owned(),
                };
                format!("{}  {}{mark}", one.id, one.display_name)
            })
            .collect(),

        Response::Sessions(lines) if lines.is_empty() => vec!["no sessions".to_owned()],

        Response::Sessions(lines) => lines
            .iter()
            .map(|one| {
                // Both together and never one alone: a session can look stuck and have a turn running, and showing only
                // the first would read as a completion runtrol never saw.
                let stuck = if one.looks_stuck {
                    "  (looks stuck)"
                } else {
                    ""
                };
                let tier = if one.hot { "running" } else { "idle" };
                // The provider's own name for the conversation, which is the one argument a resume takes. It was on the
                // wire and not on this surface, which meant a listing showed a session and withheld the only thing
                // needed to pick it back up. A session the provider has not named yet prints a placeholder rather than
                // an empty column, so the shape of a line never depends on how far along a session is.
                let native = one.native.as_deref().unwrap_or(NOT_NAMED_YET);
                format!(
                    "{}  {}  {}  {}  {native}{stuck}",
                    one.session, one.provider, tier, one.doing
                )
            })
            .collect(),

        Response::Started { session } => vec![session.to_string()],

        Response::Done => vec!["done".to_owned()],

        // As the provider wrote it. Reformatting would be this surface reading a conversation, which is the one thing
        // runtrol does not do.
        Response::Event(payload) => vec![payload.as_str().to_owned()],

        Response::Failed(failure) => {
            let mut lines = vec![failure.message.to_string()];
            if failure.needs_the_operator {
                lines.push("this needs you at the machine runtrol is running on".to_owned());
            } else if failure.retryable {
                lines.push("this may work if you try again".to_owned());
            }
            lines
        }

        // An answer from a daemon newer than this build. Shown rather than dropped: the operator asked for something and
        // deserves to know an answer came back, even one this build cannot lay out.
        other => vec![format!("an answer this build cannot lay out: {other:?}")],
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::SessionId;

    use super::*;

    #[test]
    fn a_provider_this_build_cannot_serve_is_shown_and_marked() {
        let response = Response::Welcome {
            wire: runtrol_ipc::WIRE_VERSION,
            providers: vec![
                runtrol_ipc::wire::ProviderLine {
                    id: "claude".into(),
                    display_name: "Claude Code".into(),
                    usable: true,
                    why_not: None,
                },
                runtrol_ipc::wire::ProviderLine {
                    id: "other".into(),
                    display_name: "Other".into(),
                    usable: false,
                    why_not: Some("this build has no driver for that protocol".into()),
                },
            ],
        };
        let lines = render(&response);
        assert_eq!(lines.len(), 2, "both are shown");
        assert!(lines.iter().any(|line| line.contains("no driver")));
    }

    #[test]
    fn a_session_that_looks_stuck_is_shown_as_running_and_stuck() {
        // Showing only the first would read as a completion runtrol never saw; showing only the second would hide that
        // work is still going.
        let response = Response::Sessions(vec![runtrol_ipc::wire::SessionLine {
            session: SessionId::now(),
            provider: "claude".into(),
            native: None,
            hot: true,
            doing: "busy".into(),
            looks_stuck: true,
        }]);
        let lines = render(&response);
        let line = lines.first().expect("one line");
        assert!(line.contains("busy"), "{line}");
        assert!(line.contains("looks stuck"), "{line}");
    }

    #[test]
    fn an_empty_list_says_so_rather_than_printing_nothing() {
        // Printing nothing is indistinguishable from a command that failed silently.
        assert_eq!(render(&Response::Sessions(vec![])), vec!["no sessions"]);
    }

    #[test]
    fn a_failure_that_needs_the_operator_says_where_to_go() {
        // The one honest answer a remote surface can give: authentication is unfixable from anywhere else, because
        // runtrol carries no credential.
        let lines = render(&Response::Failed(runtrol_ipc::wire::WireError {
            message: "the provider wants you to authenticate".into(),
            retryable: false,
            needs_the_operator: true,
        }));
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .get(1)
                .is_some_and(|line| line.contains("at the machine"))
        );
    }

    #[test]
    fn a_failure_worth_retrying_says_so_and_one_that_is_not_stays_quiet() {
        let retryable = render(&Response::Failed(runtrol_ipc::wire::WireError {
            message: "temporarily unavailable".into(),
            retryable: true,
            needs_the_operator: false,
        }));
        assert!(retryable.iter().any(|line| line.contains("try again")));

        let final_failure = render(&Response::Failed(runtrol_ipc::wire::WireError {
            message: "no provider called nothing".into(),
            retryable: false,
            needs_the_operator: false,
        }));
        assert_eq!(
            final_failure.len(),
            1,
            "no advice where there is none to give"
        );
    }

    #[test]
    fn an_event_is_printed_as_the_provider_wrote_it() {
        let payload = r#"{"z":1,"a":[2,3]}"#;
        let lines = render(&Response::Event(runtrol_provider::Opaque::owned(
            payload.to_owned(),
        )));
        assert_eq!(lines, vec![payload]);
    }

    #[test]
    fn a_listing_shows_the_name_a_resume_takes() {
        // The axis says start, resume and delete all happen from the one list. A resume takes the provider's own name
        // for the conversation, so a listing that shows a session and withholds that name shows a session nobody can
        // pick back up. It was on the wire and missing from this surface.
        let response = Response::Sessions(vec![runtrol_ipc::wire::SessionLine {
            session: SessionId::now(),
            provider: "codex".into(),
            native: Some("019fb614-c96e-7ce0-9d37-c0cc962e30c6".into()),
            hot: true,
            doing: "idle".into(),
            looks_stuck: false,
        }]);
        let line = render(&response).first().cloned().expect("one line");
        assert!(
            line.contains("019fb614-c96e-7ce0-9d37-c0cc962e30c6"),
            "a resume cannot be typed from this line: {line}"
        );
    }

    #[test]
    fn a_session_the_provider_has_not_named_keeps_the_same_shape() {
        // A placeholder rather than an empty column, so anything reading this surface can split on whitespace without
        // caring how far along a session is.
        let response = Response::Sessions(vec![runtrol_ipc::wire::SessionLine {
            session: SessionId::now(),
            provider: "claude".into(),
            native: None,
            hot: false,
            doing: "detached".into(),
            looks_stuck: false,
        }]);
        let line = render(&response).first().cloned().expect("one line");
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 5, "{line}");
        assert_eq!(fields.last(), Some(&NOT_NAMED_YET), "{line}");
    }

    #[test]
    fn a_listing_prints_one_session_per_line() {
        // So another program can read it too. A surface only a person can read is a surface nothing can build on.
        let response = Response::Sessions(vec![
            runtrol_ipc::wire::SessionLine {
                session: SessionId::now(),
                provider: "claude".into(),
                native: None,
                hot: true,
                doing: "idle".into(),
                looks_stuck: false,
            },
            runtrol_ipc::wire::SessionLine {
                session: SessionId::now(),
                provider: "claude".into(),
                native: None,
                hot: false,
                doing: "detached".into(),
                looks_stuck: false,
            },
        ]);
        let lines = render(&response);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(!line.contains('\n'), "one session, one line: {line}");
        }
    }
}
