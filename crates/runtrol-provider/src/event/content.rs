//! Conversation content, carried and not read.
//!
//! Every type here is an envelope. The fields runtrol lifts out are exactly the ones it takes a decision
//! on, and everything else rides inside an [`Opaque`] that reaches the subscriber unaltered.
//!
//! # The test a field has to pass
//!
//! > A field earns a place of its own if and only if the supervisor takes a decision on its value.
//!
//! The sharpest case is a tool call's title. It is a required field in the standard vocabulary and the
//! phone needs it, and runtrol **still** does not lift it, because runtrol takes no decision on it. It
//! rides in the payload and the phone renders it. If a future feature wants a title, the answer is not to
//! lift the field; it is to notice that the subscriber already has it.
//!
//! Permanently opaque, even where the standard declares it required: message text in every
//! representation, reasoning and thinking text, tool call titles and raw input and raw output, diffs,
//! terminal bytes, patch bodies, plan entry text, command descriptions, session titles, and every
//! provider error string.

use serde::Serialize;

use crate::event::Opaque;
use crate::id::{MessageId, ToolCallId};
use crate::time::WallMs;

/// A piece of a message: whole, or a fragment of one.
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    /// Which logical message this belongs to, when the provider says.
    pub message_id: Option<MessageId>,
    /// A fragment to append, rather than a whole message to show.
    ///
    /// A subscriber appends rather than replaces. A fragment also carries the current live source boundary,
    /// while a complete body advances it. Reconnect uses a separate bounded `WatchCursor`.
    pub delta: bool,
    /// The tool call this content belongs under, for subagent nesting.
    ///
    /// Routing only. runtrol never reads what the subagent said.
    pub parent: Option<ToolCallId>,
    /// The provider's content block, untouched.
    pub content: Opaque,
}

/// A tool call starting, or an update to one already started.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallFrame {
    /// The provider's identifier for this call.
    pub tool_call_id: ToolCallId,
    /// What kind of tool it is, when the provider classifies its own tools.
    ///
    /// `None` when it does not. runtrol does **not** infer a kind from a tool name: a name-to-kind table
    /// is exactly the hardcoding the discovery rule forbids, and it would be silently wrong the first
    /// time a vendor renamed a tool. One CLI classifies and gets a kind; the other does not and gets
    /// `None`, and the asymmetry is visible rather than papered over.
    pub kind: Option<ToolKind>,
    /// How the call is going.
    pub status: Option<ToolCallStatus>,
    /// A fragment of an update rather than a whole one.
    pub delta: bool,
    /// Title, content, locations, raw input, raw output, diffs, terminal bytes: all here, all unread.
    pub payload: Opaque,
}

/// What a tool does, as the provider itself classifies it.
///
/// Lifted because runtrol takes a decision on it: the risk class that gates high-risk approval authority
/// is derived partly from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ToolKind {
    /// Reads something.
    Read,
    /// Changes a file.
    Edit,
    /// Removes something.
    Delete,
    /// Moves or renames something.
    Move,
    /// Searches.
    Search,
    /// Runs a command.
    Execute,
    /// Thinks.
    Think,
    /// Fetches from the network.
    Fetch,
    /// Switches the agent's mode.
    SwitchMode,
    /// Something else. The catch-all is a distinct value rather than a default that pretends.
    Other,
}

impl ToolKind {
    /// Whether saying yes to this could change the machine or run code on it.
    ///
    /// One of the inputs to the risk class. Deliberately conservative: a kind runtrol does not recognize
    /// counts as dangerous, because the alternative is treating an unfamiliar capability as harmless.
    #[must_use]
    pub const fn is_dangerous(&self) -> bool {
        match self {
            Self::Execute | Self::Edit | Self::Delete | Self::Move => true,
            // `Other` is not listed as dangerous here on purpose: an unrecognized kind is escalated by
            // the approval risk rules, which see the whole request rather than only its kind, and
            // marking every unknown tool dangerous here would make every read look like a command.
            Self::Read
            | Self::Search
            | Self::Think
            | Self::Fetch
            | Self::SwitchMode
            | Self::Other => false,
        }
    }
}

/// How a tool call is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ToolCallStatus {
    /// Not started.
    Pending,
    /// Running.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Finished badly. A notification trigger: this is one of the few things worth waking a phone for.
    Failed,
    /// Withdrawn before finishing.
    Cancelled,
}

/// How much of the context window is in use.
///
/// A gauge, not a running total. The standard defines the used figure as tokens currently in context, so
/// it goes down after a compaction.
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    /// Tokens currently in context, when the provider reports a comparable number.
    ///
    /// `None` when it does not, and runtrol does **not** derive one by arithmetic. Summing a provider's
    /// own breakdown is interpretation, and it would be silently wrong the first time the vendor added a
    /// token category. A missing number is shown as missing.
    pub used: Option<u64>,
    /// The size of the context window.
    pub size: Option<u64>,
    /// What it has cost so far, when the provider says.
    pub cost: Option<Cost>,
    /// The provider's own usage breakdown, verbatim.
    pub detail: Opaque,
}

/// Money spent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cost {
    /// How much.
    pub amount: f64,
    /// In what. Carried as the provider wrote it, never converted.
    pub currency: Box<str>,
}

/// Where the account stands against its limits.
///
/// The one place runtrol carries something the standard vocabulary has no type for. That is allowed
/// because a quota gauge is not conversation content: runtrol owns session identity, process state, and
/// cursors, and an account limit is account state. Both supported CLIs push this for free, so the
/// alternative is not thinner, only a second request.
///
/// It also buys something concrete. A first probe on this machine stalled for 71.8 seconds and the reason
/// was invisible. With this frame it reads as "waiting on a limit" instead of a spinner.
#[derive(Debug, Clone, Serialize)]
pub struct RateLimit {
    /// The shorter window, when the provider reports one.
    pub primary: Option<Window>,
    /// The longer window, when the provider reports one.
    pub secondary: Option<Window>,
    /// A limit is blocking right now.
    pub reached: bool,
    /// The provider's own limit report, verbatim.
    pub detail: Opaque,
}

/// One rate limit window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Window {
    /// How much of the window is used, as a percentage, when the provider reports one.
    ///
    /// Optional because it is not universal. Measured on a real turn: one CLI reports which window governs and
    /// when it resets while saying nothing about how full it is. Requiring a number here forced that report to be
    /// dropped whole, which read as "no limit exists" precisely when the provider was talking about one.
    pub used_percent: Option<u8>,
    /// When it resets, when the provider says.
    pub resets_at: Option<WallMs>,
    /// How long the window is, in minutes.
    pub window_minutes: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fragment_and_a_whole_message_are_distinguishable() {
        // A subscriber appends one and replaces the other. Getting it wrong duplicates the text.
        let whole = Chunk {
            message_id: None,
            delta: false,
            parent: None,
            content: Opaque::owned(r#"{"type":"text","text":"done"}"#.to_owned()),
        };
        assert!(!whole.delta);

        let fragment = Chunk {
            message_id: None,
            delta: true,
            parent: None,
            content: Opaque::owned(r#"{"type":"text_delta","text":"do"}"#.to_owned()),
        };
        assert!(fragment.delta);
    }

    #[test]
    fn an_unclassified_tool_stays_unclassified() {
        // Inferring a kind from a tool name would be a hardcoded name table, and it would be wrong the
        // first time a vendor renamed a tool.
        let unclassified = ToolCallFrame {
            tool_call_id: ToolCallId::new("toolu_01").expect("valid id"),
            kind: None,
            status: Some(ToolCallStatus::InProgress),
            delta: false,
            payload: Opaque::owned(r#"{"name":"Bash","input":{"command":"ls"}}"#.to_owned()),
        };
        assert!(
            unclassified.kind.is_none(),
            "the payload says Bash, and runtrol still does not conclude Execute"
        );
    }

    #[test]
    fn dangerous_kinds_are_the_ones_that_change_things() {
        for kind in [
            ToolKind::Execute,
            ToolKind::Edit,
            ToolKind::Delete,
            ToolKind::Move,
        ] {
            assert!(kind.is_dangerous(), "{kind:?} changes the machine");
        }
        for kind in [
            ToolKind::Read,
            ToolKind::Search,
            ToolKind::Think,
            ToolKind::Fetch,
            ToolKind::SwitchMode,
            ToolKind::Other,
        ] {
            assert!(!kind.is_dangerous(), "{kind:?} does not, on its own");
        }
    }

    #[test]
    fn a_missing_usage_number_is_missing_rather_than_computed() {
        // One CLI reports last-turn and cumulative breakdowns rather than a context total. Summing them
        // would be interpretation, and it would break the first time a token category was added.
        let usage = Usage {
            used: None,
            size: Some(200_000),
            cost: None,
            detail: Opaque::owned(r#"{"last":{"input":10,"output":20},"total":{}}"#.to_owned()),
        };
        assert!(usage.used.is_none());
        assert_eq!(usage.size, Some(200_000));
    }

    #[test]
    fn a_content_payload_never_reaches_a_log_line() {
        // The whole content plane exists to carry text runtrol does not read. A debug format that
        // printed it would undo that in one line of logging.
        let chunk = Chunk {
            message_id: None,
            delta: false,
            parent: None,
            content: Opaque::owned(r#"{"text":"my private question"}"#.to_owned()),
        };
        let printed = format!("{chunk:?}");
        assert!(!printed.contains("private"), "content leaked: {printed}");
    }

    #[test]
    fn content_serializes_with_its_payload_verbatim() {
        let original = r#"{"type":"text","text":"hi"}"#;
        let chunk = Chunk {
            message_id: Some(MessageId::new("msg_01").expect("valid id")),
            delta: false,
            parent: None,
            content: Opaque::owned(original.to_owned()),
        };
        let encoded = serde_json::to_string(&chunk).expect("serializable");
        assert!(encoded.contains(original), "payload altered: {encoded}");
        assert!(encoded.contains(r#""message_id":"msg_01""#));
    }
}
