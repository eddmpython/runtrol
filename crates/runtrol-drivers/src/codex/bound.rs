//! THE surface runtrol binds on this CLI, and nothing else.
//!
//! Drift exposure is proportional to what a wrapper consumes, not to what a vendor ships. Measured from the
//! generated schema on this machine: **127 requests runtrol may call, one lifecycle notification it sends, 70
//! notifications it may receive, and 11 requests the provider makes of runtrol.** The four lists below are what
//! runtrol actually depends on, so the answer to "what breaks if the vendor changes something" is one file long.
//!
//! # Every request the provider makes gets an answer, including the ones runtrol cannot serve
//!
//! Eleven methods run the other way. A client that ignores one leaves the provider's daemon waiting forever, and
//! because the daemon multiplexes every session, one unanswered question stalls all of them rather than one.
//!
//! So the rule is total: an incoming request is always answered. [`REQUESTS`] lists the ones runtrol answers
//! deliberately, and anything absent from it is answered with a protocol error saying runtrol has no binding.
//! Silence is never an option here.
//!
//! # Why an approval still has a fallback answer
//!
//! A recognized approval is routed to its session and waits for a bounded human response. If the session is gone,
//! its queue is full, or its parameters cannot be represented honestly, the reader must still answer. It declines
//! in those failure paths so one abandoned question cannot stall the daemon shared by every session.

/// A method runtrol calls on the provider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundCall {
    /// The method name.
    pub method: &'static str,
    /// What runtrol uses it for, for a person reading this list.
    pub means: &'static str,
}

/// The method that opens the connection.
///
/// Measured: it is answered in roughly four seconds on a cold start, and nothing else may be called before it.
pub const HANDSHAKE: &str = "initialize";

/// A client-to-provider notification runtrol sends without expecting an answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundReport {
    /// The method name.
    pub method: &'static str,
    /// What runtrol uses it for, for a person reading this list.
    pub means: &'static str,
}

/// Acknowledge the successful handshake before making any other call.
///
/// The app-server lifecycle requires this notification after the `initialize` answer. Sending it before the answer
/// races initialization, and omitting it leaves later calls outside the initialized protocol state.
pub const INITIALIZED: &str = "initialized";

/// Every client notification runtrol sends.
pub const REPORTS: &[BoundReport] = &[BoundReport {
    method: INITIALIZED,
    means: "acknowledge the initialize answer before runtrol makes any other provider call",
}];

/// The method that ends a turn early.
///
/// Named on its own because it needs the provider's identifier for the running turn, which is the one piece of
/// per-turn state the driver has to keep.
pub const INTERRUPT: &str = "turn/interrupt";

/// Every method runtrol calls.
///
/// Ten or so of one hundred and twenty six. Listing models, threads and the account is discovery rather than
/// session work, so it belongs to the probe and not here: a driver that called them would widen this surface
/// for something no session needs.
pub const CALLS: &[BoundCall] = &[
    BoundCall {
        method: HANDSHAKE,
        means: "open the connection. once per process, before anything else",
    },
    BoundCall {
        method: "thread/start",
        means: "begin a conversation. the answer carries the provider's own identifier for it",
    },
    BoundCall {
        method: "thread/resume",
        means: "continue a conversation the provider already has",
    },
    BoundCall {
        method: "turn/start",
        means: "send what the operator wrote. answered in milliseconds with an identifier, not with the work",
    },
    BoundCall {
        method: INTERRUPT,
        means: "ask the running turn to stop. what ends it is still the provider's own word",
    },
    BoundCall {
        method: "thread/unsubscribe",
        means: "stop following a conversation without stopping the daemon other sessions are on",
    },
];

/// A notification runtrol has a binding for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundNotice {
    /// The method name.
    pub method: &'static str,
    /// Whether it names the conversation it belongs to.
    ///
    /// Almost all of them do, which is what makes one connection serving many sessions possible at all. The
    /// exception is account state, which is true of every session at once.
    pub per_thread: bool,
    /// What runtrol does about it.
    pub means: &'static str,
}

/// The notification that ends a turn.
///
/// # Why this constant exists
///
/// Measured: `turn/start` is answered in **two milliseconds** with an empty item list, and the turn then runs
/// for eight seconds. A probe that read that answer as the result reported an eight second turn as finished in
/// hundredths of a second. This is the frame that actually ends it, and naming it once means the driver and a
/// drift gate cannot end up watching different frames.
pub const TERMINAL: &str = "turn/completed";

/// The notification that says work has begun.
pub const STARTED: &str = "turn/started";

/// Every notification runtrol binds. Everything else is relayed whole and unread.
pub const NOTICES: &[BoundNotice] = &[
    BoundNotice {
        method: STARTED,
        per_thread: true,
        means: "work has demonstrably begun on a turn",
    },
    BoundNotice {
        method: TERMINAL,
        per_thread: true,
        means: "the turn ended, and the status inside says how",
    },
    BoundNotice {
        method: "item/agentMessage/delta",
        per_thread: true,
        means: "a fragment of what the agent is saying",
    },
    BoundNotice {
        method: "item/reasoning/textDelta",
        per_thread: true,
        means: "a fragment of what the agent is thinking, which renders somewhere else entirely",
    },
    BoundNotice {
        method: "item/reasoning/summaryTextDelta",
        per_thread: true,
        means: "a fragment of the summarized thinking, which renders with the thinking",
    },
    BoundNotice {
        method: "item/started",
        per_thread: true,
        means: "a piece of the turn began. the item inside says what kind",
    },
    BoundNotice {
        method: "item/completed",
        per_thread: true,
        means: "a piece of the turn finished, and this is what the provider persists",
    },
    BoundNotice {
        method: "item/fileChange/patchUpdated",
        per_thread: true,
        means: "the current file patch used to make a later approval subject complete",
    },
    BoundNotice {
        method: "thread/tokenUsage/updated",
        per_thread: true,
        means: "how much of the context window is in use",
    },
    BoundNotice {
        method: "thread/settings/updated",
        per_thread: true,
        means: "the thread's applied settings; the model inside is the CLI's word on what answers",
    },
    BoundNotice {
        method: "account/rateLimits/updated",
        // The one that names no conversation. It is account state, so it is true of every session on this
        // connection at once, and it is delivered to all of them rather than guessed at one.
        per_thread: false,
        means: "where the account stands against its limits, pushed for free on every turn",
    },
    BoundNotice {
        method: "error",
        per_thread: true,
        means: "the provider reported a failure, and the retry flag says whether the turn is still running",
    },
];

/// How runtrol answers a question the provider asks it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// Answered with a refusal that is a value the protocol has.
    ///
    /// A decline is a legitimate answer to an approval, so the provider continues rather than failing.
    Decline,
    /// Answered with a protocol error, because runtrol will not serve it.
    ///
    /// Used where no legitimate answer exists: runtrol holds no provider credential, so a request for one has
    /// no honest reply other than saying so.
    Refuse,
}

/// A request the provider makes of runtrol, and how runtrol answers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundRequest {
    /// The method name.
    pub method: &'static str,
    /// How runtrol answers it.
    pub answer: Answer,
    /// Why that is the answer.
    pub because: &'static str,
}

/// The field an approval answer carries, and the value that declines.
///
/// Measured from the generated schema: the two approval answers this build sends both carry one field named
/// this, whose declining value is the one below.
pub const DECISION_FIELD: &str = "decision";

/// The declining value of an approval decision.
pub const DECISION_DECLINE: &str = "decline";

/// The complete native result used when a routed approval must fail closed.
pub const DECLINE_RESULT: &str = r#"{"decision":"decline"}"#;

/// Every request runtrol answers deliberately.
///
/// Anything not here is still answered, with a protocol error. That is the whole point of the list: it says
/// which refusals are considered, not which requests are allowed to go unanswered.
pub const REQUESTS: &[BoundRequest] = &[
    BoundRequest {
        method: "item/commandExecution/requestApproval",
        answer: Answer::Decline,
        because: "a routed approval falls back to decline if its session cannot receive or represent it safely",
    },
    BoundRequest {
        method: "item/fileChange/requestApproval",
        answer: Answer::Decline,
        because: "the same, and a missing file-change join may be refused but never approved blind",
    },
    BoundRequest {
        method: "account/chatgptAuthTokens/refresh",
        answer: Answer::Refuse,
        because: "runtrol holds no provider credential and will not proxy one. staying silent would hang the daemon",
    },
];

/// What runtrol answers a request it has no binding for.
///
/// A protocol error rather than silence. The daemon multiplexes every session, so a question left open stalls
/// all of them.
pub const UNBOUND_REQUEST_CODE: i64 = -32_601;

/// The message runtrol sends with [`UNBOUND_REQUEST_CODE`].
pub const UNBOUND_REQUEST_MESSAGE: &str = "runtrol has no binding for that request";

/// How runtrol answers a bound request, when it does.
#[must_use]
pub fn answer_for(method: &str) -> Option<Answer> {
    REQUESTS
        .iter()
        .find(|request| request.method == method)
        .map(|request| request.answer)
}

/// Whether a provider question is a human approval this driver can route.
#[must_use]
pub fn is_approval(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
    )
}

/// Whether a notification names the conversation it belongs to.
///
/// `None` for a notification with no binding, which is relayed rather than routed.
#[must_use]
pub fn is_per_thread(method: &str) -> Option<bool> {
    NOTICES
        .iter()
        .find(|notice| notice.method == method)
        .map(|notice| notice.per_thread)
}

/// This CLI's part in cross-consult wiring.
///
/// Measured on 0.146.0:
///
/// - Registration is official: `codex mcp add <name> -- <command...>`, with `remove` and `get` beside it.
///   Its configuration is global, so there is no scope word to bind.
/// - Serving is official and consultable: `codex mcp-server` answers `tools/list` with a `codex` tool that
///   runs a session, which is exactly what a counterpart calls to get this CLI's opinion. The name is
///   verified against a fresh `tools/list` before every wiring, so a vendor rename becomes a refusal at the
///   toggle rather than a failure mid-turn.
pub const CONSULT: crate::consult::ConsultSurface = crate::consult::ConsultSurface {
    registrar: Some(crate::consult::McpRegistrar {
        add: &["mcp", "add"],
        remove: &["mcp", "remove"],
        get: &["mcp", "get"],
        get_suffix: &["--json"],
        readback: crate::consult::McpReadback::Json,
    }),
    server: Some(crate::consult::McpConsultServer {
        serve: &["mcp-server"],
        tool: crate::consult::ConsultTool::Named("codex"),
    }),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_surface_is_a_fraction_of_what_the_vendor_ships() {
        // Measured: 126 methods, 70 notifications, 11 requests. What a vendor change can break is only what
        // runtrol consumes, and keeping that small is the direct answer to how the last project in this space
        // died.
        assert!(CALLS.len() <= 12, "the call list grew to {}", CALLS.len());
        assert!(
            REPORTS.len() <= 2,
            "the client notification list grew to {}",
            REPORTS.len()
        );
        assert!(
            NOTICES.len() <= 16,
            "the notification list grew to {}",
            NOTICES.len()
        );
        assert!(
            REQUESTS.len() <= 8,
            "the answered-request list grew to {}",
            REQUESTS.len()
        );
    }

    #[test]
    fn the_frame_that_ends_a_turn_is_not_the_answer_to_starting_one() {
        // The probe bug, as a test. `turn/start` answers in two milliseconds with nothing in it; this is the
        // frame that ends the turn eight seconds later.
        assert_eq!(TERMINAL, "turn/completed");
        assert!(
            CALLS.iter().any(|call| call.method == "turn/start"),
            "starting a turn is a call"
        );
        assert!(
            NOTICES.iter().any(|notice| notice.method == TERMINAL),
            "and ending one is a notification, which is the whole distinction"
        );
        assert!(
            !CALLS.iter().any(|call| call.method == TERMINAL),
            "nothing may call the ending, because runtrol does not decide it"
        );
    }

    #[test]
    fn exactly_one_bound_notification_names_no_conversation() {
        // One connection serves every session, so routing depends on a frame naming its thread. The single
        // exception is account state, which is true of all of them at once; a second one would mean a frame
        // whose destination has to be guessed.
        let global: Vec<&BoundNotice> =
            NOTICES.iter().filter(|notice| !notice.per_thread).collect();
        assert_eq!(global.len(), 1, "{global:?}");
        assert_eq!(
            global.first().map(|notice| notice.method),
            Some("account/rateLimits/updated")
        );
    }

    #[test]
    fn nothing_is_bound_twice() {
        // Two entries for one name means two answers about what it means, and whichever is found first wins
        // silently.
        for (index, call) in CALLS.iter().enumerate() {
            for other in CALLS.iter().skip(index + 1) {
                assert_ne!(call.method, other.method, "{} is bound twice", call.method);
            }
        }
        for (index, report) in REPORTS.iter().enumerate() {
            for other in REPORTS.iter().skip(index + 1) {
                assert_ne!(
                    report.method, other.method,
                    "{} is bound twice",
                    report.method
                );
            }
        }
        for (index, notice) in NOTICES.iter().enumerate() {
            for other in NOTICES.iter().skip(index + 1) {
                assert_ne!(
                    notice.method, other.method,
                    "{} is bound twice",
                    notice.method
                );
            }
        }
        for (index, request) in REQUESTS.iter().enumerate() {
            for other in REQUESTS.iter().skip(index + 1) {
                assert_ne!(
                    request.method, other.method,
                    "{} is bound twice",
                    request.method
                );
            }
        }
    }

    #[test]
    fn every_binding_says_what_it_is_for() {
        // The list is read by a person deciding whether a vendor change matters. An entry with no explanation
        // makes that decision impossible.
        for call in CALLS {
            assert!(call.means.len() > 20, "{call:?} says nothing");
            assert!(!call.method.is_empty());
        }
        for report in REPORTS {
            assert!(report.means.len() > 20, "{report:?} says nothing");
            assert!(!report.method.is_empty());
        }
        for notice in NOTICES {
            assert!(notice.means.len() > 20, "{notice:?} says nothing");
        }
        for request in REQUESTS {
            assert!(request.because.len() > 20, "{request:?} says nothing");
        }
    }

    #[test]
    fn a_credential_request_is_refused_and_never_declined() {
        // A decline is a legitimate protocol value that says "no, carry on". There is no such answer to being
        // asked for a token runtrol does not have, so it has to be an error: answering "no, carry on" would
        // tell the daemon a refresh was considered and rejected rather than that nobody here holds one.
        assert_eq!(
            answer_for("account/chatgptAuthTokens/refresh"),
            Some(Answer::Refuse)
        );
        assert_eq!(
            answer_for("item/commandExecution/requestApproval"),
            Some(Answer::Decline)
        );
    }

    #[test]
    fn a_request_with_no_binding_is_not_on_the_answered_list() {
        // It is still answered, with a protocol error. This asserts the list means what it says: it names the
        // deliberate answers, not the set of questions that get one.
        assert_eq!(answer_for("mcpServer/elicitation/request"), None);
        assert_eq!(answer_for("attestation/generate"), None);
        assert!(UNBOUND_REQUEST_MESSAGE.len() > 20);
    }

    #[test]
    fn a_notification_nobody_bound_is_not_claimed_to_be_routable() {
        assert_eq!(is_per_thread("turn/completed"), Some(true));
        assert_eq!(is_per_thread("account/rateLimits/updated"), Some(false));
        assert_eq!(is_per_thread("thread/realtime/sdp"), None);
    }
}
