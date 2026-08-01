//! One normalized event, and the four planes it can belong to.
//!
//! # What runtrol reads, and what it refuses to read
//!
//! The thin rule needs a mechanical test, not a feeling. Here it is:
//!
//! > **A field earns a place of its own if and only if the supervisor takes a decision on its value.**
//!
//! There are exactly eight decisions the supervisor takes, and nothing else may lift a field:
//!
//! | Decision | What it has to read |
//! |---|---|
//! | Which session and turn is this | session id, epoch, turn id, tool call id, parent |
//! | Is the turn over, and who said so | terminal signal, stop reason, declarant |
//! | Does a human have to answer, and by when | approval id, options, option kinds, deadline, digest |
//! | Where can a subscriber restart from | stream incarnation, attach epoch, dense sequence |
//! | Should the phone buzz | notice level and code, failed tool status, blocked turn |
//! | Is the session healthy or degraded | notice code, retryable, exit status, detach reason |
//! | Is the account blocked on a quota | usage gauge, rate limit windows |
//! | Is this a fragment or a whole message | the delta flag, message id |
//!
//! Everything else is opaque, permanently, including fields the standard vocabulary declares required.
//!
//! # The four planes
//!
//! - **Supervisor.** runtrol's own vocabulary: attaching, turn lifecycle, notices, detaching.
//! - **Content.** The standard's session-update variants, one for one, payloads untouched.
//! - **Consent.** Approvals, which the two providers shape differently enough to need their own plane.
//! - **Nothing was dropped.** [`Unmapped`], which is what makes vendor drift a non-event.
//!
//! The content plane is deliberately a mirror of the standard rather than a design of runtrol's own. A
//! provider that already speaks that standard needs no translation table at all, and that is the test of
//! whether the normalization is real.

mod approval;
mod attach;
mod content;
mod notice;
mod opaque;
mod turn;
mod unmapped;

pub use approval::{
    ApprovalKind, ApprovalOption, ApprovalRequest, OfferedOption, PermissionOptionKind, RiskClass,
    WithdrawnReason,
};
pub use attach::{Attached, CapabilitySet, DetachReason, Detached};
pub use content::{Chunk, Cost, RateLimit, ToolCallFrame, ToolCallStatus, ToolKind, Usage, Window};
pub use notice::{Level, Notice, NoticeCode};
pub use opaque::Opaque;
pub use turn::{BlockedOn, Declarant, StopReason, TurnEvent};
pub use unmapped::Unmapped;

use serde::{Deserialize, Serialize};

use crate::id::{ApprovalId, MessageId, SessionId, StreamId};
use crate::time::WallMs;

/// One event, as it enters the session hub.
///
/// The driver supplies everything except `seq`, which the hub stamps at the single point where a driver's
/// output enters a session. That is deliberate: a driver translating one provider line into three events
/// should not have to reason about numbering, and two drivers on one session across a reattach must not
/// collide.
#[derive(Debug, Clone, Serialize)]
pub struct AgentEvent {
    /// Which session.
    pub session: SessionId,
    /// Which driver attach.
    ///
    /// Increments on every attach. `seq` is dense within one epoch and meaningless across a change of it,
    /// so a subscriber seeing the epoch move knows, structurally, to fall back to the source cursor.
    ///
    /// Density across a daemon restart is impossible without either an fsync per event or a lie. runtrol
    /// takes neither. This is the same mechanism as a database timeline identifier, and it is the only way
    /// to have real gap detection without paying for durability on every event.
    pub epoch: u32,
    /// Position within the epoch. Dense and gapless, assigned by the hub, never by a driver.
    pub seq: u64,
    /// When runtrol saw it.
    ///
    /// runtrol's clock, not the provider's, because a provider's timestamps are not monotone across a
    /// daemon restart and this field is used for ordering.
    pub at: WallMs,
    /// How far into the provider's own event source this event corresponds to.
    ///
    /// Monotone within an epoch. The unit is the driver's business. The core compares this diagnostic position and
    /// never interprets it, which keeps provider-specific branching out of the core.
    ///
    /// A fragment carries the same value as the previous durable frame, because fragments are not
    /// persisted by the provider.
    pub src_end: u64,
    /// What happened.
    pub body: EventBody,
}

impl AgentEvent {
    /// Build one positioned event.
    #[must_use]
    pub fn new(
        session: SessionId,
        epoch: u32,
        seq: u64,
        at: WallMs,
        src_end: u64,
        body: EventBody,
    ) -> Self {
        Self {
            session,
            epoch,
            seq,
            at,
            src_end,
            body,
        }
    }
}

/// The next event a watcher expects inside one live attachment.
///
/// This is a boundary rather than the last delivered event. Sequence zero therefore names a valid empty stream, and
/// reconnect arithmetic never has to add one or give a special meaning to the largest integer.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WatchCursor {
    /// The live hub incarnation. A daemon restart creates a new value even when epoch and sequence restart at zero.
    pub stream: StreamId,
    /// Which driver attachment owns the sequence.
    pub epoch: u32,
    /// The next dense sequence number the watcher needs.
    pub seq: u64,
}

/// A requested watch boundary that the bounded replay window cannot reach.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WatchGap {
    /// The boundary the watcher requested.
    pub requested: WatchCursor,
    /// The first boundary from which live delivery can continue now.
    pub live_at: WatchCursor,
}

/// What happened, across the four planes.
///
/// `#[non_exhaustive]` because a driver written outside this repository matches on this, and adding a
/// variant must not break its build.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
#[non_exhaustive]
pub enum EventBody {
    // ---- supervisor plane: runtrol's own vocabulary ----
    /// A driver bound to the provider session.
    Attached(Box<Attached>),
    /// Something happened to a turn.
    Turn(TurnEvent),
    /// A condition worth reporting that is not conversation.
    Notice(Box<Notice>),
    /// The driver is no longer bound.
    Detached(Detached),
    // ---- content plane: the standard vocabulary, one variant per discriminator ----
    /// Something the operator said.
    UserMessageChunk(Chunk),
    /// Something the agent said.
    AgentMessageChunk(Chunk),
    /// Something the agent thought.
    AgentThoughtChunk(Chunk),
    /// A tool call beginning.
    ToolCall(ToolCallFrame),
    /// A tool call progressing or finishing.
    ToolCallUpdate(ToolCallFrame),
    /// The agent's plan.
    Plan {
        /// The provider's plan object, untouched.
        payload: Opaque,
    },
    /// The commands this session offers.
    AvailableCommandsUpdate {
        /// The provider's command catalogue update, untouched.
        payload: Opaque,
    },
    /// The agent switched mode.
    CurrentModeUpdate {
        /// Which mode, by the provider's own name for it.
        mode_id: Box<str>,
        /// The provider's whole mode object.
        payload: Opaque,
    },
    /// A configuration option changed.
    ConfigOptionUpdate {
        /// The provider's configuration option update, untouched.
        payload: Opaque,
    },
    /// Session metadata changed.
    SessionInfoUpdate {
        /// The provider's session information update, untouched.
        payload: Opaque,
    },
    /// How much of the context window is in use.
    UsageUpdate(Box<Usage>),

    // ---- the one place runtrol exceeds the standard ----
    /// Where the account stands against its limits.
    ///
    /// The standard has no type for this. Justified in [`RateLimit`]: a quota gauge is account state, not
    /// conversation, and both providers push it for free.
    RateLimitUpdate(Box<RateLimit>),

    // ---- consent plane ----
    /// A human has to choose.
    ApprovalRequested(Box<ApprovalRequest>),
    /// A pending choice is gone, and nobody here made it.
    ApprovalWithdrawn {
        /// Which prompt.
        id: ApprovalId,
        /// Why it went away.
        why: WithdrawnReason,
    },

    // ---- nothing was dropped ----
    /// A frame runtrol has no binding for, carried through whole.
    Unmapped(Unmapped),
}

impl EventBody {
    /// The stable envelope discriminator used on runtrol's own event wire.
    ///
    /// This never reads an opaque provider payload. It exists so a serialization failure can name the envelope that
    /// failed without formatting conversation content.
    #[must_use]
    pub const fn wire_name(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::Turn(_) => "turn",
            Self::Notice(_) => "notice",
            Self::Detached(_) => "detached",
            Self::UserMessageChunk(_) => "userMessageChunk",
            Self::AgentMessageChunk(_) => "agentMessageChunk",
            Self::AgentThoughtChunk(_) => "agentThoughtChunk",
            Self::ToolCall(_) => "toolCall",
            Self::ToolCallUpdate(_) => "toolCallUpdate",
            Self::Plan { .. } => "plan",
            Self::AvailableCommandsUpdate { .. } => "availableCommandsUpdate",
            Self::CurrentModeUpdate { .. } => "currentModeUpdate",
            Self::ConfigOptionUpdate { .. } => "configOptionUpdate",
            Self::SessionInfoUpdate { .. } => "sessionInfoUpdate",
            Self::UsageUpdate(_) => "usageUpdate",
            Self::RateLimitUpdate(_) => "rateLimitUpdate",
            Self::ApprovalRequested(_) => "approvalRequested",
            Self::ApprovalWithdrawn { .. } => "approvalWithdrawn",
            Self::Unmapped(_) => "unmapped",
        }
    }

    /// Whether this frame is conversation content rather than supervision.
    ///
    /// The question a subscriber asks to decide whether to render into the conversation or into the status
    /// area. Also the question a future audit log asks to decide what it must **not** record.
    #[must_use]
    pub const fn is_content(&self) -> bool {
        matches!(
            self,
            Self::UserMessageChunk(_)
                | Self::AgentMessageChunk(_)
                | Self::AgentThoughtChunk(_)
                | Self::ToolCall(_)
                | Self::ToolCallUpdate(_)
                | Self::Plan { .. }
                | Self::AvailableCommandsUpdate { .. }
                | Self::CurrentModeUpdate { .. }
                | Self::ConfigOptionUpdate { .. }
                | Self::SessionInfoUpdate { .. }
                | Self::UsageUpdate(_)
        )
    }

    /// Whether this frame is a fragment to append rather than something whole.
    ///
    /// `false` for everything that is not chunked content, because "append this" is only meaningful for
    /// content that arrives in pieces.
    #[must_use]
    pub const fn is_fragment(&self) -> bool {
        match self {
            Self::UserMessageChunk(chunk)
            | Self::AgentMessageChunk(chunk)
            | Self::AgentThoughtChunk(chunk) => chunk.delta,
            Self::ToolCall(frame) | Self::ToolCallUpdate(frame) => frame.delta,
            _ => false,
        }
    }

    /// Which message this fragment belongs to, when it is one.
    #[must_use]
    pub fn message_id(&self) -> Option<&MessageId> {
        match self {
            Self::UserMessageChunk(chunk)
            | Self::AgentMessageChunk(chunk)
            | Self::AgentThoughtChunk(chunk) => chunk.message_id.as_ref(),
            _ => None,
        }
    }

    /// How many bytes of provider payload this frame holds.
    ///
    /// The number the replay ring and the fan-out budget are counted in. Counting bytes is not reading
    /// them: this reaches into an [`Opaque`] for its length and never for its contents.
    ///
    /// Only the opaque payloads count, because they are the only part a provider can make arbitrarily
    /// large. The envelope is a fixed width, and the small text fields beside it are bounded by the
    /// provider text limit, so a budget that tracked them would be tracking a constant.
    ///
    /// The match is exhaustive on purpose. A variant added here without an entry is a compile error rather
    /// than a payload that silently counts as nothing, which is how a bounded buffer stops being bounded.
    #[must_use]
    pub fn payload_bytes(&self) -> usize {
        match self {
            Self::Attached(attached) => attached.payload.len(),
            Self::Notice(notice) => notice.payload.len(),
            Self::UserMessageChunk(chunk)
            | Self::AgentMessageChunk(chunk)
            | Self::AgentThoughtChunk(chunk) => chunk.content.len(),
            Self::ToolCall(frame) | Self::ToolCallUpdate(frame) => frame.payload.len(),
            Self::Plan { payload }
            | Self::AvailableCommandsUpdate { payload }
            | Self::ConfigOptionUpdate { payload }
            | Self::SessionInfoUpdate { payload }
            | Self::CurrentModeUpdate { payload, .. } => payload.len(),
            Self::UsageUpdate(usage) => usage.detail.len(),
            Self::RateLimitUpdate(limit) => limit.detail.len(),
            Self::ApprovalRequested(request) => request.subject.len(),
            Self::Unmapped(unmapped) => unmapped.payload.len(),
            // runtrol's own frames. Every field is fixed width or bounded, so there is nothing here a
            // provider can grow.
            Self::Turn(_) | Self::Detached(_) | Self::ApprovalWithdrawn { .. } => 0,
        }
    }

    /// Whether this frame is worth waking a phone for.
    ///
    /// Deliberately short, and it is the union of exactly three things: a human is being asked something,
    /// a condition has stopped progress until somebody acts, or a tool call failed. Everything else is
    /// noise on a lock screen.
    #[must_use]
    pub fn deserves_a_notification(&self) -> bool {
        match self {
            // A human is being asked something, or the turn is waiting on one. Both mean nothing moves
            // until somebody looks.
            Self::ApprovalRequested(_) | Self::Turn(TurnEvent::Blocked { .. }) => true,
            Self::Notice(notice) => notice.code.deserves_a_notification(),
            Self::ToolCallUpdate(frame) | Self::ToolCall(frame) => {
                matches!(frame.status, Some(ToolCallStatus::Failed))
            }
            // A turn that died without a completion signal. The outcome is unknown, which is worth
            // knowing; a clean detach is not.
            Self::Detached(detached) => detached.in_turn.is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ToolCallId;

    fn an_event(body: EventBody) -> AgentEvent {
        AgentEvent::new(SessionId::now(), 0, 0, WallMs::now(), 0, body)
    }

    fn a_chunk(delta: bool) -> Chunk {
        Chunk {
            message_id: Some(MessageId::new("msg_01").expect("valid id")),
            delta,
            parent: None,
            content: Opaque::owned(r#"{"type":"text","text":"hello"}"#.to_owned()),
        }
    }

    #[test]
    fn an_event_fits_the_memory_contract() {
        // The hot tier keeps a 64 frame replay ring per session inside a 128 KiB budget, so the width of
        // this struct is part of that contract rather than a curiosity.
        //
        // Where the bytes go: 16 for the session id, 4 for the epoch, 8 each for the position, the
        // timestamp, and the cursor, which is 44 and pads to 48. The widest body is a chunk: two shared
        // identifiers at 16 each, a shared payload handle at 32, and a flag whose spare bits hold the
        // discriminant.
        //
        // The design notes proposed 96. That was written without the chunk arithmetic, and reaching it
        // would mean boxing the chunk, which is one heap allocation per streaming fragment: the highest
        // frequency event in the system. Trading 8 KiB across eight hot sessions for an allocation on
        // every fragment is the wrong way round, so the bound is the next power of two up and the reason
        // is written here rather than in a document nobody re-reads.
        assert!(
            size_of::<AgentEvent>() <= 128,
            "an event grew to {} bytes",
            size_of::<AgentEvent>()
        );
    }

    #[test]
    fn the_rare_rich_frames_are_boxed_and_the_hot_ones_are_not() {
        // Boxing is decided by frequency, not by size alone. A frame that arrives once per attach can
        // afford an allocation; one that arrives thousands of times per turn cannot.
        assert!(
            size_of::<EventBody>() <= size_of::<Chunk>() + 8,
            "the widest body should be a chunk, not something boxable"
        );
        assert!(size_of::<Attached>() > size_of::<EventBody>());
    }

    #[test]
    fn a_payload_is_counted_wherever_it_rides() {
        // A bounded buffer is only bounded if it can see the size of what it holds. Every variant that
        // carries provider bytes has to report them, whichever field they arrive in.
        let text = r#"{"type":"text","text":"hello"}"#;
        let carriers = vec![
            EventBody::AgentMessageChunk(a_chunk(false)),
            EventBody::ToolCall(ToolCallFrame {
                tool_call_id: ToolCallId::new("toolu_01").expect("valid id"),
                kind: None,
                status: None,
                delta: false,
                payload: Opaque::owned(text.to_owned()),
            }),
            EventBody::Plan {
                payload: Opaque::owned(text.to_owned()),
            },
            EventBody::CurrentModeUpdate {
                mode_id: "plan".into(),
                payload: Opaque::owned(text.to_owned()),
            },
            EventBody::Unmapped(Unmapped {
                tag: "whatever".into(),
                turn: None,
                payload: Opaque::owned(text.to_owned()),
                unknown_to_binding: true,
            }),
        ];
        for body in carriers {
            assert_eq!(
                body.payload_bytes(),
                text.len(),
                "{body:?} does not report the bytes it holds"
            );
        }
    }

    #[test]
    fn a_frame_runtrol_originates_holds_no_provider_bytes() {
        assert_eq!(
            EventBody::Turn(TurnEvent::Started {
                turn: crate::id::TurnId::first(0)
            })
            .payload_bytes(),
            0
        );
    }

    #[test]
    fn content_and_supervision_are_distinguishable() {
        assert!(EventBody::AgentMessageChunk(a_chunk(false)).is_content());
        assert!(
            EventBody::Plan {
                payload: Opaque::none(),
            }
            .is_content()
        );
        assert!(
            !EventBody::Turn(TurnEvent::Started {
                turn: crate::id::TurnId::first(0)
            })
            .is_content()
        );
        assert!(
            !EventBody::Unmapped(Unmapped {
                tag: "whatever".into(),
                turn: None,
                payload: Opaque::none(),
                unknown_to_binding: true,
            })
            .is_content(),
            "an unmapped frame is not classified as content, because runtrol does not know what it is"
        );
    }

    #[test]
    fn fragments_are_distinguishable_from_whole_messages() {
        assert!(EventBody::AgentMessageChunk(a_chunk(true)).is_fragment());
        assert!(!EventBody::AgentMessageChunk(a_chunk(false)).is_fragment());
        assert!(
            !EventBody::Plan {
                payload: Opaque::none(),
            }
            .is_fragment()
        );
    }

    #[test]
    fn a_notification_fires_for_a_human_being_needed() {
        let blocked = EventBody::Turn(TurnEvent::Blocked {
            turn: crate::id::TurnId::first(0),
            on: BlockedOn::UserInput,
        });
        assert!(blocked.deserves_a_notification());

        let failed = EventBody::ToolCallUpdate(ToolCallFrame {
            tool_call_id: ToolCallId::new("toolu_01").expect("valid id"),
            kind: Some(ToolKind::Execute),
            status: Some(ToolCallStatus::Failed),
            delta: false,
            payload: Opaque::none(),
        });
        assert!(failed.deserves_a_notification());

        let died_mid_turn = EventBody::Detached(Detached {
            reason: DetachReason::ProcessExit,
            exit: Some(1),
            in_turn: Some(crate::id::TurnId::first(0)),
        });
        assert!(died_mid_turn.deserves_a_notification());
    }

    #[test]
    fn a_notification_does_not_fire_for_ordinary_progress() {
        // A phone that buzzes on every fragment is a phone with notifications disabled.
        assert!(!EventBody::AgentMessageChunk(a_chunk(true)).deserves_a_notification());
        assert!(
            !EventBody::Plan {
                payload: Opaque::none(),
            }
            .deserves_a_notification()
        );
        assert!(
            !EventBody::Detached(Detached {
                reason: DetachReason::Requested,
                exit: None,
                in_turn: None,
            })
            .deserves_a_notification(),
            "a clean detach is not news"
        );
    }

    #[test]
    fn an_event_serializes_with_its_envelope_and_its_payload_intact() {
        let payload = r#"{"type":"text","text":"hello"}"#;
        let event = an_event(EventBody::AgentMessageChunk(a_chunk(false)));
        let encoded = serde_json::to_string(&event).expect("serializable");
        assert!(
            encoded.contains(r#""event":"agentMessageChunk""#),
            "{encoded}"
        );
        assert!(encoded.contains(payload), "payload altered: {encoded}");
        assert!(encoded.contains(r#""epoch":0"#));
        assert!(encoded.contains(r#""src_end":0"#));
    }

    #[test]
    fn a_whole_event_never_reaches_a_log_line_with_its_content() {
        // The realistic leak is `tracing::debug!("{event:?}")` written during an investigation. It has to
        // be safe by construction, not by convention.
        let event = an_event(EventBody::AgentMessageChunk(Chunk {
            message_id: None,
            delta: false,
            parent: None,
            content: Opaque::owned(r#"{"text":"my private question"}"#.to_owned()),
        }));
        let printed = format!("{event:?}");
        assert!(!printed.contains("private"), "leaked: {printed}");
    }
}
