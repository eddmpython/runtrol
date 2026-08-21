//! THE surface runtrol binds on this CLI, and nothing else.
//!
//! Drift exposure is proportional to what a wrapper consumes, not to what a vendor ships. This file is the whole
//! list, so the answer to "what breaks if the vendor changes something" is one file long, and a drift gate has
//! exactly one thing to watch.
//!
//! # Measured on this machine, version 2.1.220
//!
//! One turn, captured from a session runtrol itself started and prompted, with the frames it produced in order:
//!
//! | `type` | `subtype` | what it is |
//! |---|---|---|
//! | `system` | `init` | the session started, with its capability list |
//! | `system` | `status` | progress that is not conversation |
//! | `rate_limit_event` | | where the account stands, pushed without being asked |
//! | `stream_event` | | a fragment, with the real kind nested one level down |
//! | `assistant` | | a whole message |
//! | `result` | `success` | **the turn ended**, with the answer in a `result` field |
//!
//! # This frame has now been got wrong twice, and how it was caught the second time
//!
//! A recorded design note said `result`. A reading of a capture then said `message`/`success`, and this file was
//! changed to match it. That reading was wrong: the frame is `type: "result"` with `subtype: "success"`, and
//! `result` is also a *field* inside it, which is what the misreading confused it with.
//!
//! Nothing caught it until the product was run. A session was started, prompted, and watched, and the ending came
//! out of the far end tagged `result/success` and marked as something runtrol has no binding for. The turn had
//! finished and runtrol did not know, which is precisely the failure this file exists to prevent.
//!
//! The lesson is not about this frame. It is that a fixture written by hand proves only that the code agrees with
//! whoever wrote the fixture: the tests below were green throughout, because they were written against the same
//! misreading. The fixtures here are now copied from frames the product actually received.
//!
//! # Why the terminal frame is named twice
//!
//! [`TERMINAL`] is the pair the driver matches on, and it is also what a drift gate compares against a fresh
//! observation. One constant, two readers, so the gate cannot watch a different frame than the driver uses.

/// A frame kind runtrol has a binding for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundFrame {
    /// The `type` field.
    pub kind: &'static str,
    /// The `subtype` field, when the binding depends on one.
    pub subtype: Option<&'static str>,
    /// What runtrol does about it, for a person reading this list.
    pub means: &'static str,
}

/// The frame that ends a turn.
///
/// # Why no subtype, when the one that was measured is `success`
///
/// Because the subtype says *how* the turn ended, not *whether* it did. Binding the pair would mean a turn that
/// ended any other way ends nothing at all: the frame would travel as unmapped, the session would sit at running
/// forever, and the one case where an operator most needs to be told is the one that would go silent. So the kind
/// is what ends a turn, and the subtype is read alongside `is_error` and `stop_reason` to say how.
///
/// `success` is the only subtype observed here. That is a fact about one turn on one machine, not a claim that
/// there are no others, and binding the kind is what makes the difference harmless.
pub const TERMINAL: BoundFrame = BoundFrame {
    kind: "result",
    subtype: None,
    means: "the turn ended. the subtype and the error flag say how, and the answer is a field inside it",
};

/// Every frame runtrol binds. Everything else is relayed whole and unread.
pub const FRAMES: &[BoundFrame] = &[
    BoundFrame {
        kind: "system",
        subtype: Some("init"),
        means: "the session started. carries the capability list and the identifier runtrol issued",
    },
    TERMINAL,
    BoundFrame {
        kind: "assistant",
        subtype: None,
        means: "a whole message from the agent",
    },
    BoundFrame {
        kind: "user",
        subtype: None,
        means: "a whole message from the operator, echoed back",
    },
    BoundFrame {
        kind: "stream_event",
        subtype: None,
        means: "a fragment of a message, with its real kind nested one level down",
    },
    BoundFrame {
        kind: "rate_limit_event",
        subtype: None,
        means: "where the account stands against its limits, pushed for free",
    },
];

/// The control channel, which is a surface and not an event mapping.
///
/// Measured: a `control_request` is answered with a `control_response` **before any session starts**, so the
/// channel is alive without a turn. That is how an interrupt is acknowledged and how an approval reaches a person.
///
/// Kept apart from [`FRAMES`] because requests on this channel require an answer, not only a one-way event mapping.
/// The driver binds approval requests and cancellations, consumes responses to its own requests, and fails closed
/// on every other provider question.
pub const CONTROL: &[BoundFrame] = &[
    BoundFrame {
        kind: "control_request",
        subtype: None,
        means: "the CLI asking runtrol something, which is how an approval reaches a person",
    },
    BoundFrame {
        kind: "control_response",
        subtype: None,
        means: "an answer on the control channel, which is how an interrupt is acknowledged",
    },
    BoundFrame {
        kind: "control_cancel_request",
        subtype: None,
        means: "the CLI withdrawing a provider question before runtrol answered it",
    },
];

/// The flags runtrol passes, and what it does when one is missing.
///
/// Separate from the frame list because they drift separately and are confirmed differently: a frame is observed
/// in a stream, and a flag is confirmed by asking the CLI's own argument parser.
pub type BoundFlag = crate::kinds::DriverFlag;

/// Every flag runtrol depends on.
pub const FLAGS: &[BoundFlag] = &[
    BoundFlag {
        flag: "--print",
        required: true,
        without_it: "the CLI runs its own interface instead of speaking a protocol",
    },
    BoundFlag {
        flag: "--input-format",
        required: true,
        without_it: "runtrol cannot send a turn as structured input",
    },
    BoundFlag {
        flag: "--output-format",
        required: true,
        without_it: "runtrol cannot read the session as structured output",
    },
    BoundFlag {
        flag: "--session-id",
        required: true,
        without_it: "runtrol cannot issue the identifier, and a resume would need a path lookup instead",
    },
    BoundFlag {
        flag: "--resume",
        required: true,
        without_it: "a session can be started and never continued",
    },
    BoundFlag {
        flag: "--verbose",
        required: true,
        without_it: "the CLI refuses to stream structured output at all",
    },
    BoundFlag {
        flag: "--include-partial-messages",
        required: false,
        without_it: "a message appears all at once instead of as it is written",
    },
    // Measured on 2.1.238: with this flag the CLI re-emits each message it read from stdin as a `user` frame
    // (`"isReplay":true`) before it answers. It is how the operator's own words reach the conversation through
    // the provider's mouth rather than a local echo, which is the only way they are shown at all.
    BoundFlag {
        flag: "--replay-user-messages",
        required: false,
        without_it: "the operator's own messages are not shown in the conversation, only the replies",
    },
    BoundFlag {
        flag: "--permission-mode",
        required: false,
        without_it: "the session runs at whatever permission the operator's own settings say",
    },
    // Measured on 2.1.220: this flag exists and its own help does not list it. Confirmed by asking the parser
    // with a control group. A capability check that read help would conclude it is absent and quietly disable
    // approval prompts, which is why the probe asks the parser instead.
    BoundFlag {
        flag: "--permission-prompt-tool",
        required: true,
        without_it: "approvals cannot be brokered, so a remote session cannot start safely",
    },
    BoundFlag {
        flag: "--model",
        required: false,
        without_it: "the session runs whatever model the operator's own settings choose",
    },
    BoundFlag {
        flag: "--effort",
        required: false,
        without_it: "the session runs at whatever reasoning effort the operator's own settings choose",
    },
];

/// This CLI's part in cross-consult wiring.
///
/// Measured on 2.1.220:
///
/// - Registration is official: `claude mcp add --scope user <name> -- <command...>`, with `remove` and `get`
///   beside it. User scope is bound deliberately, because it is the one scope whose canonical file the CLI
///   itself names on every change, which is what lets a smoke assert the mutation is exactly one entry.
/// - Serving is official (`claude mcp serve`) but there is nothing to consult: `tools/list` answers with the
///   CLI's own toolset, and the one delegating tool in it answers "Agent type 'general-purpose' not found"
///   over an empty available list in serve context. Declaring the absence keeps the reverse direction an
///   honest "unsupported" instead of a wiring that fails mid-turn.
pub const CONSULT: crate::consult::ConsultSurface = crate::consult::ConsultSurface {
    registrar: Some(crate::consult::McpRegistrar {
        add: &["mcp", "add", "--scope", "user"],
        remove: &["mcp", "remove", "--scope", "user"],
        get: &["mcp", "get"],
        get_suffix: &[],
        readback: crate::consult::McpReadback::LabeledText,
    }),
    server: Some(crate::consult::McpConsultServer {
        serve: &["mcp", "serve"],
        tool: crate::consult::ConsultTool::Absent {
            why: "this CLI's own MCP server exposes its toolset, not a consultation: measured on 2.1.220, \
                  its delegating tool answers 'Agent type not found' with an empty available list in serve \
                  context",
        },
    }),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_surface_is_small_enough_to_read() {
        // Drift exposure is proportional to what runtrol consumes. This CLI has 61 flags and an open-ended set
        // of frame kinds; the list here is what a vendor change can actually break.
        assert!(
            FRAMES.len() <= 12,
            "the frame list grew to {}",
            FRAMES.len()
        );
        assert!(
            CONTROL.len() <= 4,
            "the control list grew to {}",
            CONTROL.len()
        );
        assert!(FLAGS.len() <= 16, "the flag list grew to {}", FLAGS.len());
    }

    #[test]
    fn the_terminal_frame_is_the_one_the_product_received() {
        // Copied from a session runtrol started, prompted, and watched, after a hand-written fixture had this
        // wrong and every test agreed with it. A driver that misses this never sees a turn end, and the operator
        // watches a finished turn spin with nothing saying why.
        assert_eq!(TERMINAL.kind, "result");
        assert_eq!(
            TERMINAL.subtype, None,
            "the subtype says how a turn ended, not whether it did, so a turn that ended badly must still end"
        );
        assert!(
            !FRAMES.iter().any(|frame| frame.kind == "message"),
            "there is no frame of that kind, and binding one would be binding the misreading again"
        );
    }

    #[test]
    fn the_terminal_frame_is_in_the_list_exactly_once() {
        // One constant with two readers: the driver matches on it and a drift gate compares it against a fresh
        // observation. A second copy would let the gate watch a different frame than the driver uses.
        let terminal: Vec<&BoundFrame> = FRAMES
            .iter()
            .filter(|frame| frame.kind == TERMINAL.kind && frame.subtype == TERMINAL.subtype)
            .collect();
        assert_eq!(terminal.len(), 1, "{terminal:?}");
    }

    #[test]
    fn no_frame_is_bound_twice() {
        // Two entries for one frame means two answers to what it means, and whichever is matched first wins
        // silently.
        let all: Vec<&BoundFrame> = FRAMES.iter().chain(CONTROL).collect();
        for (index, frame) in all.iter().enumerate() {
            for other in all.iter().skip(index + 1) {
                assert!(
                    frame.kind != other.kind || frame.subtype != other.subtype,
                    "{frame:?} is bound twice"
                );
            }
        }
    }

    #[test]
    fn every_binding_says_what_it_is_for() {
        // The list is read by a person deciding whether a vendor change matters. An entry with no explanation
        // makes that decision impossible.
        for frame in FRAMES.iter().chain(CONTROL) {
            assert!(!frame.means.is_empty(), "{frame:?} says nothing");
            assert!(!frame.kind.is_empty());
        }
        for flag in FLAGS {
            assert!(!flag.without_it.is_empty(), "{flag:?} says nothing");
            assert!(flag.flag.starts_with("--"), "{flag:?}");
        }
    }

    #[test]
    fn every_flag_that_is_not_required_says_what_degrades() {
        // A missing optional flag degrades a feature. The driver uses this text when an explicit choice depends on
        // that flag, so an option without it could be dropped without an actionable refusal.
        for flag in FLAGS.iter().filter(|flag| !flag.required) {
            assert!(
                flag.without_it.len() > 20,
                "{:?} needs a sentence an operator can act on",
                flag.flag
            );
        }
    }

    #[test]
    fn the_flags_a_session_cannot_start_without_are_the_ones_that_were_measured() {
        // Measured: without the print flag the CLI runs its own interface, and without the verbose flag it
        // refuses to stream structured output at all. Those are start-or-not facts, and the rest degrade.
        let required: Vec<&str> = FLAGS
            .iter()
            .filter(|flag| flag.required)
            .map(|flag| flag.flag)
            .collect();
        assert!(required.contains(&"--print"));
        assert!(required.contains(&"--verbose"));
        assert!(required.contains(&"--session-id"));
        assert!(required.contains(&"--permission-prompt-tool"));
    }
}
