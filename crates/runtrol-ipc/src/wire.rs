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

use runtrol_provider::{Opaque, ProviderError, SessionId};
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

    /// The sessions, in the order the daemon lists them.
    Sessions(Vec<SessionLine>),

    /// A session was started or resumed.
    Started {
        /// runtrol's own name for it.
        session: SessionId,
    },

    /// Done, with nothing to say about it.
    Done,

    /// One event, already encoded.
    ///
    /// Encoded once by the daemon and handed to every watcher, so three watchers cost one encode. Also the last hop a
    /// conversation takes, and nothing here reads it.
    Event(Opaque),

    /// It did not work.
    Failed(WireError),
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

/// One session, as a listing shows it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionLine {
    /// runtrol's own name.
    pub session: SessionId,
    /// Which CLI it belongs to.
    pub provider: Box<str>,
    /// The provider's own name, once it has announced one.
    pub native: Option<Box<str>>,
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
        let about = [
            Request::Prompt {
                session,
                text: "do the thing".into(),
            },
            Request::Interrupt { session },
            Request::Watch { session },
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
    fn a_request_reads_back_as_what_it_was() {
        let session = SessionId::now();
        for request in [
            Request::Hello { wire: WIRE_VERSION },
            Request::List,
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
            Request::StopEverything,
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
    fn a_pass_through_payload_survives_the_tagged_envelope() {
        // Measured: with the tag inside the content this does not encode at all. The encoder has to buffer the content
        // into a model so it can add a field to it, and buffering a payload is the re-reading the design avoids. The
        // failure is loud rather than quiet, which is the better way to find out.
        let payload = r#"{"z":1,"a":[2,3],"nested":{"k":"v"}}"#;
        let encoded = serde_json::to_string(&Response::Event(Opaque::owned(payload.to_owned())))
            .expect("a tag beside the content lets a payload through");
        assert!(encoded.contains(payload), "byte for byte: {encoded}");

        let back: Response = serde_json::from_str(&encoded).expect("and it reads back");
        match back {
            Response::Event(read) => assert_eq!(read.as_str(), payload),
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn an_event_crosses_as_bytes_nobody_re_reads() {
        // The last hop a conversation takes. Encoded once by the daemon and handed to every watcher, so three watchers
        // cost one encode rather than three.
        let event =
            Opaque::owned(r#"{"event":"agentMessageChunk","content":{"text":"hello"}}"#.to_owned());
        let response = Response::Event(event);
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
