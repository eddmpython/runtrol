//! Turning an answer into the lines a person reads.
//!
//! One thing per line, so that a listing is something another program can read as well as a person. A surface only a
//! person can read is a surface nothing can build on, and this is the surface every script somebody writes around
//! runtrol will be written against.
//!
//! Each event is preceded by its runtrol-owned reconnect boundary, then printed as the provider wrote it. Laying the
//! payload out differently would be this surface reading a conversation, which is the one thing runtrol does not do.

use runtrol_ipc::wire::Response;
use runtrol_provider::ModelCatalog;

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

        Response::Sessions(listing) => {
            let mut rendered = listing
                .sessions
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
                .collect::<Vec<_>>();
            if rendered.is_empty() {
                rendered.push("no sessions".to_owned());
            }
            rendered.extend(
                listing
                    .warnings
                    .iter()
                    .map(|warning| format!("warning  {warning}")),
            );
            rendered
        }

        Response::Models(catalogue) => render_models(catalogue),

        Response::Started { session } => vec![session.to_string()],

        Response::Done => vec!["done".to_owned()],

        Response::Watching { live_at, gap, .. } => {
            render_watching(*live_at, gap.as_deref().copied())
        }

        // The cursor line is runtrol-owned transport metadata. The second line remains exactly what the provider wrote:
        // reformatting it would be this surface reading a conversation, which is the one thing runtrol does not do.
        Response::Event {
            payload,
            next_expected,
        } => render_event(payload, *next_expected),

        Response::Lagged { next_expected } => vec![format!(
            "watch lagged  reconnect after {}:{}:{}",
            next_expected.stream, next_expected.epoch, next_expected.seq
        )],

        Response::Consult(directions) => render_consult(directions),

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

fn render_models(catalogue: &ModelCatalog) -> Vec<String> {
    match catalogue {
        ModelCatalog::Known { models } if models.is_empty() => {
            vec!["no models reported".to_owned()]
        }
        ModelCatalog::Known { models } => models
            .iter()
            .flat_map(|model| {
                let default = if model.is_default { "  (default)" } else { "" };
                std::iter::once(format!("{}  {}{default}", model.id, model.display_name)).chain(
                    model
                        .reasoning_efforts
                        .iter()
                        .map(|effort| format!("effort  {}  {}", model.id, effort.id)),
                )
            })
            .collect(),
        ModelCatalog::Aliases {
            aliases,
            reasoning_efforts,
            why,
        } => {
            let mut lines = vec![format!("aliases only: {why}")];
            lines.extend(aliases.iter().map(|alias| format!("alias  {alias}")));
            lines.extend(
                reasoning_efforts
                    .iter()
                    .map(|effort| format!("effort  {}", effort.id)),
            );
            lines
        }
        ModelCatalog::Partial {
            aliases,
            models,
            reasoning_efforts,
            why,
        } => {
            let mut lines = vec![format!("partial catalogue: {why}")];
            lines.extend(aliases.iter().map(|alias| format!("alias  {alias}")));
            lines.extend(
                reasoning_efforts
                    .iter()
                    .map(|effort| format!("effort  {}", effort.id)),
            );
            lines.extend(models.iter().flat_map(|model| {
                let default = if model.is_default { "  (default)" } else { "" };
                std::iter::once(format!(
                    "model  {}  {}{default}",
                    model.id, model.display_name
                ))
                .chain(
                    model
                        .reasoning_efforts
                        .iter()
                        .map(|effort| format!("effort  {}  {}", model.id, effort.id)),
                )
            }));
            lines
        }
        ModelCatalog::Unknown { why } => vec![format!("model catalogue unknown: {why}")],
        ModelCatalog::Unsupported { why } => {
            vec![format!("model catalogue unsupported: {why}")]
        }
        other => vec![format!("model catalogue unknown to this build: {other:?}")],
    }
}

fn render_consult(directions: &[runtrol_ipc::wire::ConsultLine]) -> Vec<String> {
    if directions.is_empty() {
        return vec!["no consult directions".to_owned()];
    }
    directions
        .iter()
        .map(|one| {
            let state = match one.state {
                runtrol_ipc::wire::ConsultState::Wired => "wired",
                runtrol_ipc::wire::ConsultState::Unwired => "unwired",
                runtrol_ipc::wire::ConsultState::Unsupported => "unsupported",
            };
            // Fixed fields first, prose last, so the line splits on whitespace like every other listing on
            // this surface.
            let why = one
                .why
                .as_deref()
                .map(|why| format!("  ({why})"))
                .unwrap_or_default();
            format!("consult  {}  {}  {state}{why}", one.from, one.to)
        })
        .collect()
}

fn render_event(
    payload: &runtrol_provider::Opaque,
    next_expected: runtrol_provider::WatchCursor,
) -> Vec<String> {
    vec![
        format!(
            "watch event  next {}:{}:{}",
            next_expected.stream, next_expected.epoch, next_expected.seq
        ),
        payload.as_str().to_owned(),
    ]
}

fn render_watching(
    live_at: runtrol_provider::WatchCursor,
    gap: Option<runtrol_provider::WatchGap>,
) -> Vec<String> {
    let boundary = format!("{}:{}:{}", live_at.stream, live_at.epoch, live_at.seq);
    let mut lines = vec![format!("watching  {boundary}")];
    if let Some(gap) = gap {
        lines.push(format!(
            "watch gap  requested {}:{}:{}  live {}:{}:{}",
            gap.requested.stream,
            gap.requested.epoch,
            gap.requested.seq,
            gap.live_at.stream,
            gap.live_at.epoch,
            gap.live_at.seq
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{
        ModelCatalog, ModelChoice, ReasoningChoice, SessionId, StreamId, WatchCursor, WatchGap,
    };

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
            device: None,
            push_public_key: None,
            build_digest: None,
        };
        let lines = render(&response);
        assert_eq!(lines.len(), 2, "both are shown");
        assert!(lines.iter().any(|line| line.contains("no driver")));
    }

    #[test]
    fn a_session_that_looks_stuck_is_shown_as_running_and_stuck() {
        // Showing only the first would read as a completion runtrol never saw; showing only the second would hide that
        // work is still going.
        let response = Response::Sessions(runtrol_ipc::wire::SessionListing {
            sessions: vec![runtrol_ipc::wire::SessionLine {
                session: SessionId::now(),
                provider: "claude".into(),
                native: None,
                label: None,
                workspace: "C:\\work".into(),
                hot: true,
                doing: "busy".into(),
                waiting_on: Some(runtrol_ipc::wire::SessionWaiting::Person),
                looks_stuck: true,
            }],
            warnings: Vec::new(),
            usage: Vec::new(),
        });
        let lines = render(&response);
        let line = lines.first().expect("one line");
        assert!(line.contains("busy"), "{line}");
        assert!(line.contains("looks stuck"), "{line}");
    }

    #[test]
    fn an_empty_list_says_so_rather_than_printing_nothing() {
        // Printing nothing is indistinguishable from a command that failed silently.
        assert_eq!(
            render(&Response::Sessions(
                runtrol_ipc::wire::SessionListing::default()
            )),
            vec!["no sessions"]
        );
    }

    #[test]
    fn a_watch_acknowledgement_names_the_subscription_boundary() {
        let live_at = WatchCursor {
            stream: StreamId::now(),
            epoch: 3,
            seq: 8,
        };
        assert_eq!(
            render(&Response::Watching {
                starts_at: live_at,
                live_at,
                gap: None,
            }),
            vec![format!(
                "watching  {}:{}:{}",
                live_at.stream, live_at.epoch, live_at.seq
            )]
        );
        let requested = WatchCursor { seq: 2, ..live_at };
        let lines = render(&Response::Watching {
            starts_at: live_at,
            live_at,
            gap: Some(Box::new(WatchGap { requested, live_at })),
        });
        assert_eq!(lines.len(), 2);
        let gap_line = lines.get(1).expect("one explicit gap line");
        assert!(gap_line.contains("watch gap"));
        assert!(gap_line.contains(&requested.seq.to_string()));
    }

    #[test]
    fn discovered_models_are_one_choice_per_line() {
        let lines = render(&Response::Models(ModelCatalog::Known {
            models: vec![ModelChoice {
                id: "runtime-choice".into(),
                display_name: "Runtime Choice".into(),
                description: "reported now".into(),
                is_default: true,
                reasoning_efforts: vec![ReasoningChoice {
                    id: "provider-effort".into(),
                    description: Box::default(),
                }],
            }],
        }));
        assert_eq!(
            lines,
            vec![
                "runtime-choice  Runtime Choice  (default)",
                "effort  runtime-choice  provider-effort"
            ]
        );
    }

    #[test]
    fn aliases_and_unknown_catalogues_say_what_they_are() {
        let aliases = render(&Response::Models(ModelCatalog::Aliases {
            aliases: vec!["fast".into()],
            reasoning_efforts: vec![ReasoningChoice {
                id: "global-effort".into(),
                description: Box::default(),
            }],
            why: "aliases only".into(),
        }));
        assert!(
            aliases
                .first()
                .is_some_and(|line| line.starts_with("aliases only:"))
        );
        assert!(aliases.get(1).is_some_and(|line| line == "alias  fast"));
        assert!(
            aliases
                .get(2)
                .is_some_and(|line| line == "effort  global-effort")
        );

        let unknown = render(&Response::Models(ModelCatalog::unknown(
            "no discovery surface",
        )));
        assert!(unknown.first().is_some_and(|line| line.contains("unknown")));

        let partial = render(&Response::Models(ModelCatalog::Partial {
            aliases: vec!["fast".into()],
            models: vec![ModelChoice {
                id: "runtime-model".into(),
                display_name: "Runtime Model".into(),
                description: "provider-owned cache".into(),
                is_default: false,
                reasoning_efforts: Vec::new(),
            }],
            reasoning_efforts: vec![ReasoningChoice {
                id: "global-effort".into(),
                description: Box::default(),
            }],
            why: "partial discovery".into(),
        }));
        assert_eq!(partial.get(1).map(String::as_str), Some("alias  fast"));
        assert_eq!(
            partial.get(3).map(String::as_str),
            Some("model  runtime-model  Runtime Model")
        );
    }

    #[test]
    fn consult_directions_are_one_per_line_with_the_reason_last() {
        let lines = render(&Response::Consult(vec![
            runtrol_ipc::wire::ConsultLine {
                from: "claude".into(),
                to: "codex".into(),
                state: runtrol_ipc::wire::ConsultState::Wired,
                why: None,
            },
            runtrol_ipc::wire::ConsultLine {
                from: "codex".into(),
                to: "claude".into(),
                state: runtrol_ipc::wire::ConsultState::Unsupported,
                why: Some("measured absent".into()),
            },
        ]));
        assert_eq!(lines.len(), 2);
        let wired = lines.first().expect("two lines");
        // Fixed fields first so the line splits on whitespace like every other listing here.
        let fields: Vec<&str> = wired.split_whitespace().collect();
        assert_eq!(fields, vec!["consult", "claude", "codex", "wired"]);
        let unsupported = lines.get(1).expect("two lines");
        assert!(unsupported.contains("unsupported"), "{unsupported}");
        assert!(unsupported.contains("measured absent"), "{unsupported}");

        assert_eq!(
            render(&Response::Consult(Vec::new())),
            vec!["no consult directions"],
            "an empty answer says so rather than printing nothing"
        );
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
    fn an_event_names_its_reconnect_boundary_then_prints_the_provider_bytes_unchanged() {
        let payload = r#"{"z":1,"a":[2,3]}"#;
        let next_expected = WatchCursor {
            stream: StreamId::now(),
            epoch: 0,
            seq: 1,
        };
        let lines = render(&Response::Event {
            payload: runtrol_provider::Opaque::owned(payload.to_owned()),
            next_expected,
        });
        assert_eq!(
            lines,
            vec![
                format!(
                    "watch event  next {}:{}:{}",
                    next_expected.stream, next_expected.epoch, next_expected.seq
                ),
                payload.to_owned(),
            ]
        );
    }

    #[test]
    fn a_listing_shows_the_name_a_resume_takes() {
        // The axis says start, resume and delete all happen from the one list. A resume takes the provider's own name
        // for the conversation, so a listing that shows a session and withholds that name shows a session nobody can
        // pick back up. It was on the wire and missing from this surface.
        let response = Response::Sessions(runtrol_ipc::wire::SessionListing {
            sessions: vec![runtrol_ipc::wire::SessionLine {
                session: SessionId::now(),
                provider: "codex".into(),
                native: Some("019fb614-c96e-7ce0-9d37-c0cc962e30c6".into()),
                label: None,
                workspace: "C:\\work".into(),
                hot: true,
                doing: "idle".into(),
                waiting_on: None,
                looks_stuck: false,
            }],
            warnings: Vec::new(),
            usage: Vec::new(),
        });
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
        let response = Response::Sessions(runtrol_ipc::wire::SessionListing {
            sessions: vec![runtrol_ipc::wire::SessionLine {
                session: SessionId::now(),
                provider: "claude".into(),
                native: None,
                label: None,
                workspace: "C:\\work".into(),
                hot: false,
                doing: "detached".into(),
                waiting_on: None,
                looks_stuck: false,
            }],
            warnings: Vec::new(),
            usage: Vec::new(),
        });
        let line = render(&response).first().cloned().expect("one line");
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 5, "{line}");
        assert_eq!(fields.last(), Some(&NOT_NAMED_YET), "{line}");
    }

    #[test]
    fn a_listing_prints_one_session_per_line() {
        // So another program can read it too. A surface only a person can read is a surface nothing can build on.
        let response = Response::Sessions(runtrol_ipc::wire::SessionListing {
            sessions: vec![
                runtrol_ipc::wire::SessionLine {
                    session: SessionId::now(),
                    provider: "claude".into(),
                    native: None,
                    label: None,
                    workspace: "C:\\work".into(),
                    hot: true,
                    doing: "idle".into(),
                    waiting_on: None,
                    looks_stuck: false,
                },
                runtrol_ipc::wire::SessionLine {
                    session: SessionId::now(),
                    provider: "claude".into(),
                    native: None,
                    label: None,
                    workspace: "C:\\work".into(),
                    hot: false,
                    doing: "detached".into(),
                    waiting_on: None,
                    looks_stuck: false,
                },
            ],
            warnings: Vec::new(),
            usage: Vec::new(),
        });
        let lines = render(&response);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(!line.contains('\n'), "one session, one line: {line}");
        }
    }
}
