//! THE surface runtrol binds on this CLI, and nothing else.
//!
//! Drift exposure is proportional to what a wrapper consumes, not to what a vendor ships. This file is the whole
//! list, so the answer to "what breaks if the vendor changes something" is one file long, and a drift gate has
//! exactly one thing to watch.
//!
//! # Measured on this machine, version 2.1.220
//!
//! One turn through `-p --input-format stream-json --output-format stream-json --verbose`, with the frames it
//! produced in order:
//!
//! | `type` | `subtype` | what it is |
//! |---|---|---|
//! | `system` | `init` | the session started, with its capability list |
//! | `system` | `status` | progress that is not conversation |
//! | `system` | `thinking_tokens` | a running estimate, four times in one short turn |
//! | `rate_limit_event` | | where the account stands, pushed without being asked |
//! | `stream_event` | | a fragment, with the real kind nested one level down |
//! | `assistant` | | a whole message |
//! | `message` | `success` | **the turn ended** |
//!
//! # The measurement that refuted the design note
//!
//! The recorded design said the terminal frame is `type: "result"`. On 2.1.220 it is
//! **`type: "message"` with `subtype: "success"`**. There is no `result` frame at all; `result` is a *field*
//! inside the terminal frame.
//!
//! A driver written from the note would never see a turn end. The session would sit at "running" forever, the
//! operator would watch a finished turn spin, and nothing anywhere would say why. This is the exact failure the
//! bound-surface discipline exists to catch, caught by running the thing instead of reading about it.
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
/// Measured, not documented. See the module notes: the design said `result` and the CLI says this.
pub const TERMINAL: BoundFrame = BoundFrame {
    kind: "message",
    subtype: Some("success"),
    means: "the turn ended, and its outcome is in this frame",
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
/// Kept apart from [`FRAMES`] because nothing maps these into events yet, and a list that claimed otherwise would
/// be a binding that exists on paper and nowhere else. The mapping arrives with approvals; on that day these move
/// up, and a test below goes red to say so.
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
];

/// The flags runtrol passes, and what it does when one is missing.
///
/// Separate from the frame list because they drift separately and are confirmed differently: a frame is observed
/// in a stream, and a flag is confirmed by asking the CLI's own argument parser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundFlag {
    /// The flag.
    pub flag: &'static str,
    /// Whether a session can start at all without it.
    pub required: bool,
    /// What is lost when it is absent, for the notice an operator reads.
    pub without_it: &'static str,
}

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
        required: false,
        without_it: "approvals cannot be brokered, and the session runs at its starting permission mode",
    },
    BoundFlag {
        flag: "--model",
        required: false,
        without_it: "the session runs whatever model the operator's own settings choose",
    },
];

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
    fn the_terminal_frame_is_the_one_that_was_measured_and_not_the_one_that_was_written_down() {
        // The design note said `result`. The CLI says `message`/`success`, and `result` is a field inside it. A
        // driver written from the note would never see a turn end, and the operator would watch a finished turn
        // spin forever with nothing saying why.
        assert_eq!(TERMINAL.kind, "message");
        assert_eq!(TERMINAL.subtype, Some("success"));
        assert!(
            !FRAMES.iter().any(|frame| frame.kind == "result"),
            "the frame the note described does not exist and must not be bound"
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
        // A missing optional flag degrades a feature and announces it. That announcement is this text, so an
        // optional flag without it would degrade silently.
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
        assert!(
            !required.contains(&"--permission-prompt-tool"),
            "an undocumented flag must never be required, because it can disappear without a changelog"
        );
    }
}
