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
//! | Where can a subscriber restart from | the source cursor |
//! | Should the phone buzz | notice level and code, failed tool status, blocked turn |
//! | Is the session healthy or degraded | notice code, retryable, exit status, detach reason |
//! | Is the account blocked on a quota | usage gauge, rate limit windows |
//! | Is this a fragment or a whole message | the delta flag, message id |
//!
//! Everything else is opaque, permanently, including fields the standard vocabulary declares required.
//!
//! # The four planes
//!
//! - **Supervisor.** runtrol's own vocabulary: attaching, turn lifecycle, notices, detaching, lag.
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
pub use attach::{Attached, CapabilitySet, DetachReason, Detached, FileId, ReplaySource};
pub use content::{Chunk, Cost, RateLimit, ToolCallFrame, ToolCallStatus, ToolKind, Usage, Window};
pub use notice::{Level, Notice, NoticeCode};
pub use opaque::Opaque;
pub use turn::{BlockedOn, Declarant, StopReason, TurnEvent};
pub use unmapped::Unmapped;

use serde::Serialize;

use crate::id::{ApprovalId, MessageId, SessionId};
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
    /// How far into the provider's own store this event corresponds to.
    ///
    /// Monotone within an epoch. The unit is the driver's business: a byte offset into an append-only
    /// transcript, or whatever a bound range method counts. The core compares this and never interprets
    /// it, and that is what keeps provider-specific branching out of the core.
    ///
    /// A fragment carries the same value as the previous durable frame, because fragments are not
    /// persisted by the provider.
    pub src_end: u64,
    /// What happened.
    pub body: EventBody,
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
    /// A subscriber's queue overflowed.
    ///
    /// Its **position** was dropped; its data was not. The frame carries what it needs to recover from the
    /// provider's own store, which is only possible because runtrol keeps no copy of the data. Thinness
    /// buying correctness, at the one point where a slow phone would otherwise be able to exhaust the
    /// daemon's memory.
    Lagged {
        /// The last position that reached the subscriber.
        last_delivered_seq: u64,
        /// Where to resume from in the provider's own store.
        resume_from: u64,
    },

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
    Plan(Opaque),
    /// The commands this session offers.
    AvailableCommandsUpdate(Opaque),
    /// The agent switched mode.
    CurrentModeUpdate {
        /// Which mode, by the provider's own name for it.
        mode_id: Box<str>,
        /// The provider's whole mode object.
        payload: Opaque,
    },
    /// A configuration option changed.
    ConfigOptionUpdate(Opaque),
    /// Session metadata changed.
    SessionInfoUpdate(Opaque),
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
                | Self::Plan(_)
                | Self::AvailableCommandsUpdate(_)
                | Self::CurrentModeUpdate { .. }
                | Self::ConfigOptionUpdate(_)
                | Self::SessionInfoUpdate(_)
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
        AgentEvent {
            session: SessionId::now(),
            epoch: 0,
            seq: 0,
            at: WallMs::now(),
            src_end: 0,
            body,
        }
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
    fn content_and_supervision_are_distinguishable() {
        assert!(EventBody::AgentMessageChunk(a_chunk(false)).is_content());
        assert!(EventBody::Plan(Opaque::none()).is_content());
        assert!(
            !EventBody::Turn(TurnEvent::Started {
                turn: crate::id::TurnId::first(0)
            })
            .is_content()
        );
        assert!(
            !EventBody::Lagged {
                last_delivered_seq: 4,
                resume_from: 900,
            }
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
        assert!(!EventBody::Plan(Opaque::none()).is_fragment());
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
        assert!(!EventBody::Plan(Opaque::none()).deserves_a_notification());
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

    #[test]
    fn lag_drops_a_position_and_names_where_to_resume() {
        // The data is not dropped. It cannot be: runtrol does not own it, so recovering it means reading
        // the provider's own store from the offset this frame carries.
        let lagged = EventBody::Lagged {
            last_delivered_seq: 41,
            resume_from: 18_204,
        };
        match lagged {
            EventBody::Lagged {
                last_delivered_seq,
                resume_from,
            } => {
                assert_eq!(last_delivered_seq, 41);
                assert!(resume_from > 0, "a resume point into the provider's store");
            }
            other => panic!("expected lag, got {other:?}"),
        }
    }
}
