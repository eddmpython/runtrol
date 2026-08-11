//! What the command surface asks and what the daemon answers.
//!
//! # Every request names what it is about
//!
//! There is no current session and no selected provider. Two processes holding a notion of "the one you meant" is two
//! places for that notion to disagree, and the way it goes wrong is the worst available: a command lands on a session
//! the operator was not looking at. So every request that concerns a session carries it.
//!
//! # An event crosses as bytes that were serialized once
//!
//! [`Response::Event`] carries an event that is already encoded. The daemon encodes it once and hands the same bytes to
//! every watcher, so a session with a phone, a terminal and a window watching costs one encode rather than three. It is
//! also the last place a conversation could be re-read, and there is nothing here that could: the bytes go from a
//! provider's line to a subscriber's screen without runtrol looking inside.
//!
//! # Why the tag sits beside the content and not inside it
//!
//! Measured: an internally tagged enum cannot carry a pass-through payload at all. Putting the tag inside the content
//! forces the encoder to buffer that content into a model first so it can add a field to it, and buffering a payload is
//! exactly the re-reading this whole design exists to avoid. It fails outright rather than degrading, which is the
//! better of the two ways to find out.
//!
//! So the tag is a field beside the content. Both directions use the same shape, because two shapes on one wire is one
//! more thing for a reader to get wrong.
//!
//! # Why the error carries two flags rather than a code to look up
//!
//! A client makes exactly two decisions about a failure: whether to try again, and whether to tell the operator to go
//! to their machine. Those are on the value, so a client cannot get them wrong by branching on a number whose meaning
//! lives somewhere else.

use runtrol_provider::{
    ApprovalId, ModelCatalog, Opaque, OptionId, ProviderError, SessionId, WatchCursor, WatchGap,
};
use serde::{Deserialize, Serialize};

use crate::frame::WIRE_VERSION;

/// What the command surface asks for.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "ask", content = "with", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Request {
    /// Open the conversation and agree on a wire format.
    ///
    /// First on every connection. A side that hears a version it does not speak refuses by name rather than reading the
    /// rest with the wrong meaning.
    Hello {
        /// The wire format the caller speaks.
        wire: u8,
    },

    /// Every session this machine has.
    List,

    /// Watch the session index.
    ///
    /// The daemon sends one current [`Response::Sessions`] snapshot after the acknowledgement, then another only
    /// when a list-visible value changes. Conversation frames never enter this stream.
    WatchSessions,

    /// Discover the current model choices for one provider.
    Models {
        /// Which provider to ask. The driver owns how discovery works.
        provider: Box<str>,
    },

    /// Begin a conversation that does not exist yet.
    Start {
        /// Which CLI.
        provider: Box<str>,
        /// Where the agent works.
        workspace: Box<str>,
        /// The model to ask for, when the operator chose one.
        model: Option<Box<str>>,
        /// The permission posture to start at, when the operator chose one.
        permission: Option<Box<str>>,
    },

    /// Continue a conversation the provider already has.
    Resume {
        /// Which CLI.
        provider: Box<str>,
        /// The provider's own name for the conversation.
        native: Box<str>,
        /// Where the agent works.
        workspace: Box<str>,
    },

    /// Send what the operator wrote.
    Prompt {
        /// Which session.
        session: SessionId,
        /// What they wrote, carried and never rewritten.
        text: Box<str>,
    },

    /// Choose one option from a provider approval that is still pending.
    AnswerApproval {
        /// Which session owns the approval.
        session: SessionId,
        /// The runtrol approval identifier shown with the request.
        approval: ApprovalId,
        /// The exact provider-offered option the operator chose.
        option: OptionId,
        /// The digest shown with the subject, binding the answer to that exact content.
        subject_digest: [u8; 32],
    },

    /// Stop the turn that is running.
    ///
    /// A request, not an outcome. What ends the turn is still the provider's own word, arriving as an event.
    Interrupt {
        /// Which session.
        session: SessionId,
    },

    /// Watch a session's events.
    Watch {
        /// Which session.
        session: SessionId,
        /// The next event the caller expects, or no cursor for the bounded initial view.
        #[serde(default)]
        after: Option<WatchCursor>,
    },

    /// End a session.
    Close {
        /// Which session.
        session: SessionId,
        /// Stop it now rather than letting it finish.
        now: bool,
    },

    /// Stop every agent on this machine.
    ///
    /// Carries nothing, on purpose. The security posture requires this to work from anywhere with no permission at all,
    /// and a request with no arguments has nothing an attacker could aim. The worst it achieves is stopping work, which
    /// is the safe direction.
    StopEverything,

    /// Every cross-consult direction this build knows, with its current wired state.
    ///
    /// Read-only: the state lives in the CLIs' own configuration and is asked for fresh, so there is no second
    /// place for it to go stale.
    Consult,

    /// Register `to` as a consultable MCP server inside `from`, using `from`'s own official command.
    ConsultWire {
        /// The CLI that gains a consultant.
        from: Box<str>,
        /// The CLI whose opinion becomes reachable mid-turn.
        to: Box<str>,
    },

    /// Undo [`Request::ConsultWire`] with `from`'s own removal command, restoring its configuration.
    ConsultUnwire {
        /// The CLI that loses its consultant.
        from: Box<str>,
        /// The CLI being unregistered.
        to: Box<str>,
    },
}

/// What the daemon answers.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "say", content = "with", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Response {
    /// The wire format is agreed, and here is what this build has.
    Welcome {
        /// The wire format the daemon speaks.
        wire: u8,
        /// Every provider it knows about, usable or not.
        ///
        /// Including the ones it cannot serve. An operator with a perfectly good manifest for a kind this build has no
        /// driver for should see it marked rather than wonder where it went.
        providers: Vec<ProviderLine>,
    },

    /// The sessions and any damaged rows the daemon could not read.
    Sessions(SessionListing),

    /// The model choices one provider can honestly offer now.
    Models(ModelCatalog),

    /// A session was started or resumed.
    Started {
        /// runtrol's own name for it.
        session: SessionId,
    },

    /// Done, with nothing to say about it.
    Done,

    /// A watch subscription is installed and all later answers on this connection are events.
    Watching {
        /// The first event this response stream will deliver, or `live_at` when replay is empty.
        starts_at: WatchCursor,
        /// The exact boundary between bounded replay and the installed live subscription.
        live_at: WatchCursor,
        /// The requested boundary could not be served from the bounded window.
        gap: Option<Box<WatchGap>>,
    },

    /// A session-index subscription is installed and all later answers are current session snapshots.
    WatchingSessions,

    /// One event, already encoded, with the next exact reconnect boundary.
    ///
    /// Encoded once by the daemon and handed to every watcher, so three watchers cost one encode. Also the last hop a
    /// conversation takes, and nothing here reads it.
    Event {
        /// The original provider event envelope and opaque payload.
        payload: Opaque,
        /// The first dense event not included in this response.
        next_expected: WatchCursor,
    },

    /// This watch was retired after its bounded queue filled.
    Lagged {
        /// The first dense event the watcher did not receive.
        next_expected: WatchCursor,
    },

    /// Every cross-consult direction, each with its current state.
    ///
    /// Answered for the status request and after a wire or unwire, so a surface renders one shape and never
    /// derives state on its own.
    Consult(Vec<ConsultLine>),

    /// It did not work.
    Failed(WireError),
}

/// One cross-consult direction, as a surface shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsultLine {
    /// The CLI that would gain a consultant.
    pub from: Box<str>,
    /// The CLI whose opinion would become reachable.
    pub to: Box<str>,
    /// Where this direction stands.
    pub state: ConsultState,
    /// Why, when the state needs a sentence: the measured absence for an unsupported direction, or the
    /// CLI's own words when its answer could not be trusted.
    pub why: Option<Box<str>>,
}

/// Where one cross-consult direction stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsultState {
    /// The registration exists in the `from` CLI's own configuration.
    Wired,
    /// It does not.
    Unwired,
    /// This direction cannot be wired, and `why` says what was measured.
    Unsupported,
}

/// The small fixed edges around an already encoded event payload.
///
/// Keeping the provider-sized payload as its own slice lets the transport write it without allocating a second full
/// response and then a third framed copy. This type lives beside [`Response`] so the split wire spelling has one owner.
#[derive(Debug)]
pub struct EventResponseEdges {
    suffix: Vec<u8>,
}

impl EventResponseEdges {
    /// Bytes before the raw event payload.
    #[must_use]
    pub const fn prefix(&self) -> &'static [u8] {
        br#"{"say":"event","with":{"payload":"#
    }

    /// Bytes after the raw event payload.
    #[must_use]
    pub fn suffix(&self) -> &[u8] {
        &self.suffix
    }
}

/// Encode only the cursor-sized suffix of an event response.
///
/// # Errors
///
/// When this build cannot serialize its own reconnect cursor.
pub fn event_response_edges(
    next_expected: WatchCursor,
) -> Result<EventResponseEdges, serde_json::Error> {
    let cursor = serde_json::to_vec(&next_expected)?;
    let mut suffix = Vec::with_capacity(cursor.len() + 20);
    suffix.extend_from_slice(br#","next_expected":"#);
    suffix.extend_from_slice(&cursor);
    suffix.extend_from_slice(b"}}");
    Ok(EventResponseEdges { suffix })
}

/// One provider, as a listing shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderLine {
    /// Its identifier.
    pub id: Box<str>,
    /// What to call it in front of a person.
    pub display_name: Box<str>,
    /// Whether a session can be started for it.
    pub usable: bool,
    /// Why not, when it cannot.
    ///
    /// A sentence rather than a flag, because "this build has no driver for that protocol" and "nothing declares that
    /// kind" send the operator in different directions.
    pub why_not: Option<Box<str>>,
}

/// A session listing that can report one damaged row without hiding every readable row.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SessionListing {
    /// Readable sessions, oldest first.
    pub sessions: Vec<SessionLine>,
    /// Named storage failures that were skipped.
    pub warnings: Vec<Box<str>>,
}

/// One session, as a listing shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionLine {
    /// runtrol's own name.
    pub session: SessionId,
    /// Which CLI it belongs to.
    pub provider: Box<str>,
    /// The provider's own name, once it has announced one.
    pub native: Option<Box<str>>,
    /// Where the agent works.
    ///
    /// On the wire because a surface has to be able to say which session is touching which folder, which is
    /// the whole of the `sessions do not trample each other` axis. It is deliberately **not** on the terminal
    /// listing: that surface is whitespace-splittable by contract, and a path can contain spaces.
    pub workspace: Box<str>,
    /// How much of it exists: whether it has a process right now.
    pub hot: bool,
    /// What it is doing, in one word.
    pub doing: Box<str>,
    /// It has gone quiet, and the turn is still running.
    ///
    /// Both halves matter. A subscriber shows "this looks stuck" and offers to stop it; what it must not show is a
    /// completion runtrol invented.
    pub looks_stuck: bool,
}

/// Why something did not work.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WireError {
    /// What went wrong, in words an operator reads.
    pub message: Box<str>,
    /// Trying again could plausibly work without anybody intervening.
    pub retryable: bool,
    /// The operator has to do something at their own machine.
    ///
    /// The one honest answer a phone can give. Authentication in particular is unfixable from anywhere else, because
    /// runtrol carries no credential and a remote caller has no way to supply one.
    pub needs_the_operator: bool,
}

impl WireError {
    /// Turn a provider failure into what goes on the wire.
    ///
    /// The only place that mapping happens. Two codes for one failure would mean clients branching on the wrong one,
    /// and a retryable failure being treated as fatal.
    #[must_use]
    pub fn from_provider(error: &ProviderError) -> Self {
        Self {
            message: error.to_string().into(),
            retryable: error.retryable(),
            needs_the_operator: error.needs_operator_at_the_machine(),
        }
    }

    /// A failure that is nobody's fault but has to be reported.
    #[must_use]
    pub fn plain(message: &str) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            needs_the_operator: false,
        }
    }
}

/// Whether a hello agrees with this build.
///
/// # Errors
///
/// The version this build speaks, when they differ. A caller turns that into a message naming both, because an operator
/// whose two processes disagree needs to know which one is behind.
pub const fn agree(theirs: u8) -> Result<u8, u8> {
    if theirs == WIRE_VERSION {
        Ok(WIRE_VERSION)
    } else {
        Err(WIRE_VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_request(request: &Request) -> Request {
        let encoded = serde_json::to_string(request).expect("a request is writable");
        serde_json::from_str(&encoded).expect("and readable")
    }

    #[test]
    fn every_request_about_a_session_carries_which_one() {
        // No current session and no selected provider. Two processes holding "the one you meant" is two places for that
        // to disagree, and the way it goes wrong is a command landing on a session the operator was not looking at.
        let session = SessionId::now();
        let approval = ApprovalId::now();
        let about = [
            Request::Prompt {
                session,
                text: "do the thing".into(),
            },
            Request::Interrupt { session },
            Request::AnswerApproval {
                session,
                approval,
                option: OptionId(0),
                subject_digest: [1; 32],
            },
            Request::Watch {
                session,
                after: None,
            },
            Request::Close {
                session,
                now: false,
            },
        ];
        for request in about {
            let encoded = serde_json::to_string(&request).expect("writable");
            assert!(
                encoded.contains(&session.to_string()),
                "a request about a session must name it: {encoded}"
            );
        }
    }

    #[test]
    fn stopping_everything_carries_nothing_that_could_be_aimed() {
        // The one capability the security posture requires to work from anywhere with no permission. A request with no
        // arguments has nothing an attacker can point at, and the worst it achieves is stopping work.
        let encoded = serde_json::to_string(&Request::StopEverything).expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&encoded).expect("readable");
        let object = parsed.as_object().expect("an object");
        assert_eq!(object.len(), 1, "the tag and nothing else: {encoded}");
        assert_eq!(
            object.get("ask").and_then(|v| v.as_str()),
            Some("stopEverything")
        );
    }

    #[test]
    fn watching_sessions_is_a_payload_free_subscription_boundary() {
        assert_eq!(
            serde_json::to_string(&Request::WatchSessions).expect("writable"),
            r#"{"ask":"watchSessions"}"#
        );
        assert_eq!(
            serde_json::to_string(&Response::WatchingSessions).expect("writable"),
            r#"{"say":"watchingSessions"}"#
        );
    }

    #[test]
    fn a_request_reads_back_as_what_it_was() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        for request in [
            Request::Hello { wire: WIRE_VERSION },
            Request::List,
            Request::WatchSessions,
            Request::Models {
                provider: "claude".into(),
            },
            Request::Start {
                provider: "claude".into(),
                workspace: "/work".into(),
                model: Some("haiku".into()),
                permission: None,
            },
            Request::Resume {
                provider: "claude".into(),
                native: "some-name".into(),
                workspace: "/work".into(),
            },
            Request::Prompt {
                session,
                text: "hello".into(),
            },
            Request::AnswerApproval {
                session,
                approval,
                option: OptionId(2),
                subject_digest: [3; 32],
            },
            Request::StopEverything,
            Request::Consult,
            Request::ConsultWire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::ConsultUnwire {
                from: "claude".into(),
                to: "codex".into(),
            },
        ] {
            let back = round_trip_request(&request);
            assert_eq!(
                core::mem::discriminant(&back),
                core::mem::discriminant(&request),
                "a request changed shape crossing the wire"
            );
        }
    }

    #[test]
    fn watch_reads_an_absent_or_exact_next_expected_cursor() {
        let session = SessionId::now();
        let old_shape = format!(r#"{{"ask":"watch","with":{{"session":"{session}"}}}}"#);
        match serde_json::from_str::<Request>(&old_shape)
            .expect("an omitted cursor remains readable")
        {
            Request::Watch { after: None, .. } => {}
            other => panic!("expected an initial watch, got {other:?}"),
        }

        let after = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 7,
            seq: 91,
        };
        let encoded = serde_json::to_string(&Request::Watch {
            session,
            after: Some(after),
        })
        .expect("writable");
        match serde_json::from_str::<Request>(&encoded).expect("readable") {
            Request::Watch {
                session: read_session,
                after: Some(read_after),
            } => {
                assert_eq!(read_session, session);
                assert_eq!(read_after, after);
            }
            other => panic!("expected a cursor watch, got {other:?}"),
        }
    }

    #[test]
    fn a_prompt_carries_what_the_operator_wrote_and_nothing_added() {
        let written = "first line\nsecond line";
        let request = Request::Prompt {
            session: SessionId::now(),
            text: written.into(),
        };
        match round_trip_request(&request) {
            Request::Prompt { text, .. } => assert_eq!(&*text, written),
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[test]
    fn an_approval_answer_keeps_the_exact_subject_binding() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        let digest = core::array::from_fn(|index| u8::try_from(index).expect("the index fits"));
        let request = Request::AnswerApproval {
            session,
            approval,
            option: OptionId(17),
            subject_digest: digest,
        };

        match round_trip_request(&request) {
            Request::AnswerApproval {
                session: read_session,
                approval: read_approval,
                option,
                subject_digest,
            } => {
                assert_eq!(read_session, session);
                assert_eq!(read_approval, approval);
                assert_eq!(option, OptionId(17));
                assert_eq!(subject_digest, digest);
            }
            other => panic!("expected an approval answer, got {other:?}"),
        }
    }

    #[test]
    fn a_pass_through_payload_survives_the_tagged_envelope() {
        // Measured: with the tag inside the content this does not encode at all. The encoder has to buffer the content
        // into a model so it can add a field to it, and buffering a payload is the re-reading the design avoids. The
        // failure is loud rather than quiet, which is the better way to find out.
        let payload = r#"{"z":1,"a":[2,3],"nested":{"k":"v"}}"#;
        let next_expected = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 2,
            seq: 9,
        };
        let encoded = serde_json::to_string(&Response::Event {
            payload: Opaque::owned(payload.to_owned()),
            next_expected,
        })
        .expect("a tag beside the content lets a payload through");
        assert!(encoded.contains(payload), "byte for byte: {encoded}");

        let back: Response = serde_json::from_str(&encoded).expect("and it reads back");
        match back {
            Response::Event {
                payload: read,
                next_expected: read_next,
            } => {
                assert_eq!(read.as_str(), payload);
                assert_eq!(read_next, next_expected);
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn an_event_crosses_as_bytes_nobody_re_reads() {
        // The last hop a conversation takes. Encoded once by the daemon and handed to every watcher, so three watchers
        // cost one encode rather than three.
        let event =
            Opaque::owned(r#"{"event":"agentMessageChunk","content":{"text":"hello"}}"#.to_owned());
        let response = Response::Event {
            payload: event,
            next_expected: WatchCursor {
                stream: runtrol_provider::StreamId::now(),
                epoch: 0,
                seq: 1,
            },
        };
        let encoded = serde_json::to_string(&response).expect("writable");

        assert!(
            encoded.contains(r#""text":"hello""#),
            "the payload has to arrive unaltered: {encoded}"
        );
        let printed = format!("{response:?}");
        assert!(
            !printed.contains("hello"),
            "and it must not reach a log line: {printed}"
        );
    }

    #[test]
    fn split_event_edges_are_the_exact_response_wire_shape() {
        let payload = Opaque::owned(r#"{"text":"kept raw"}"#.to_owned());
        let next_expected = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 4,
            seq: 19,
        };
        let whole = serde_json::to_vec(&Response::Event {
            payload: payload.clone(),
            next_expected,
        })
        .expect("writable");
        let edges = event_response_edges(next_expected).expect("cursor is writable");
        let split = [edges.prefix(), payload.as_str().as_bytes(), edges.suffix()].concat();

        assert_eq!(split, whole);
        assert!(matches!(
            serde_json::from_slice::<Response>(&split).expect("readable"),
            Response::Event { .. }
        ));
    }

    #[test]
    fn watch_acknowledgements_and_lag_controls_round_trip_every_cursor() {
        let requested = WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            epoch: 3,
            seq: 8,
        };
        let live_at = WatchCursor {
            seq: 21,
            ..requested
        };
        let watching = Response::Watching {
            starts_at: live_at,
            live_at,
            gap: Some(Box::new(WatchGap { requested, live_at })),
        };
        match serde_json::from_slice::<Response>(
            &serde_json::to_vec(&watching).expect("watch acknowledgement is writable"),
        )
        .expect("watch acknowledgement is readable")
        {
            Response::Watching {
                starts_at,
                live_at: read_live,
                gap: Some(gap),
            } => {
                assert_eq!(starts_at, live_at);
                assert_eq!(read_live, live_at);
                assert_eq!(*gap, WatchGap { requested, live_at });
            }
            other => panic!("expected a watch acknowledgement, got {other:?}"),
        }

        let lagged = Response::Lagged {
            next_expected: requested,
        };
        match serde_json::from_slice::<Response>(
            &serde_json::to_vec(&lagged).expect("lag control is writable"),
        )
        .expect("lag control is readable")
        {
            Response::Lagged { next_expected } => assert_eq!(next_expected, requested),
            other => panic!("expected a lag control, got {other:?}"),
        }
    }

    #[test]
    fn a_consult_answer_carries_every_direction_with_its_state_and_reason() {
        // One shape for status and for the answer to a wire, so a surface never derives state on its own.
        let response = Response::Consult(vec![
            ConsultLine {
                from: "claude".into(),
                to: "codex".into(),
                state: ConsultState::Wired,
                why: None,
            },
            ConsultLine {
                from: "codex".into(),
                to: "claude".into(),
                state: ConsultState::Unsupported,
                why: Some("measured absent".into()),
            },
        ]);
        let encoded = serde_json::to_string(&response).expect("writable");
        assert!(encoded.contains("unsupported"), "{encoded}");
        match serde_json::from_str::<Response>(&encoded).expect("readable") {
            Response::Consult(lines) => {
                assert_eq!(lines.len(), 2);
                let unsupported = lines
                    .iter()
                    .find(|line| line.state == ConsultState::Unsupported)
                    .expect("the unsupported direction survives the wire");
                assert!(unsupported.why.is_some(), "and it says why");
            }
            other => panic!("expected a consult answer, got {other:?}"),
        }
    }

    #[test]
    fn a_provider_that_cannot_be_served_is_listed_with_the_reason() {
        // An operator with a perfectly good manifest for a kind this build has no driver for should see it marked, not
        // wonder where it went.
        let response = Response::Welcome {
            wire: WIRE_VERSION,
            providers: vec![
                ProviderLine {
                    id: "claude".into(),
                    display_name: "Claude Code".into(),
                    usable: true,
                    why_not: None,
                },
                ProviderLine {
                    id: "something".into(),
                    display_name: "Something Else".into(),
                    usable: false,
                    why_not: Some("this build has no driver for that protocol".into()),
                },
            ],
        };
        let encoded = serde_json::to_string(&response).expect("writable");
        let back: Response = serde_json::from_str(&encoded).expect("readable");
        match back {
            Response::Welcome { providers, .. } => {
                assert_eq!(providers.len(), 2, "both are listed");
                let unusable = providers
                    .iter()
                    .find(|one| !one.usable)
                    .expect("the unusable one is there");
                assert!(unusable.why_not.is_some(), "and it says why");
            }
            other => panic!("expected a welcome, got {other:?}"),
        }
    }

    #[test]
    fn a_session_that_looks_stuck_still_reports_its_turn_as_running() {
        // Both halves matter. A surface shows "this looks stuck" and offers to stop it; what it must not show is a
        // completion runtrol invented.
        let line = SessionLine {
            session: SessionId::now(),
            provider: "claude".into(),
            native: Some("some-name".into()),
            workspace: r"C:\work\dartlab".into(),
            hot: true,
            doing: "busy".into(),
            looks_stuck: true,
        };
        let encoded = serde_json::to_string(&line).expect("writable");
        let back: SessionLine = serde_json::from_str(&encoded).expect("readable");
        assert!(back.looks_stuck);
        assert_eq!(&*back.doing, "busy", "quiet is not finished");
    }

    #[test]
    fn a_failure_carries_the_two_decisions_a_client_makes() {
        // Whether to try again, and whether to send the operator to their machine. On the value, so a client cannot get
        // them wrong by branching on a number whose meaning lives somewhere else.
        let authentication = ProviderError::AuthRequired {
            provider: runtrol_provider::ProviderId::parse("claude").expect("valid"),
            how: "run the login command".to_owned(),
        };
        let wire = WireError::from_provider(&authentication);
        assert!(
            wire.needs_the_operator,
            "authentication is unfixable remotely"
        );
        assert!(wire.message.contains("login"), "{}", wire.message);

        let plain = WireError::plain("nothing is listening");
        assert!(!plain.retryable);
        assert!(!plain.needs_the_operator);
    }

    #[test]
    fn a_failure_that_could_work_next_time_says_so() {
        // A retryable failure treated as fatal is a session the operator gives up on for no reason.
        let transient = ProviderError::Spawn {
            provider: runtrol_provider::ProviderId::parse("claude").expect("valid"),
            program: "claude".to_owned(),
            source: std::io::Error::other("temporarily unavailable"),
        };
        let wire = WireError::from_provider(&transient);
        assert_eq!(wire.retryable, transient.retryable());
    }

    #[test]
    fn a_hello_that_does_not_agree_answers_with_what_this_build_speaks() {
        assert_eq!(agree(WIRE_VERSION), Ok(WIRE_VERSION));
        assert_eq!(agree(WIRE_VERSION + 1), Err(WIRE_VERSION));
        assert_eq!(agree(0), Err(WIRE_VERSION));
    }
}
