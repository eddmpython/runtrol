//! What the courier says back: a receipt for what it admitted, a refusal for what it did not.

use crate::envelope::{CallKind, PROTOCOL_VERSION, VisitedBound};
use crate::id::{CallId, ManagedSessionId, MessageId};
use crate::limits::UnixMillis;

/// Where one message is in its mechanical life.
///
/// None of these means the model read, understood, obeyed, completed, or agreed with the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Admitted to the target's mailbox.
    Accepted,
    /// The target consumed the envelope.
    Received,
    /// A correlated reply to this ask arrived.
    Replied,
    /// The deadline passed first.
    Expired,
    /// Withdrawn by its asker, or ended by a session leaving.
    Cancelled,
}

/// What `send` hands back: the message was admitted to the target's mailbox, and nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Receipt {
    /// The message that was admitted.
    pub message_id: MessageId,
    /// The call it belongs to.
    pub call_id: CallId,
    /// Always [`DeliveryState::Accepted`] at this moment. Later states are read back from the courier.
    pub state: DeliveryState,
}

/// Why an envelope was not admitted. A refused envelope leaves the courier exactly as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The envelope speaks another layout.
    #[error("protocol version {offered} where {PROTOCOL_VERSION} is required")]
    UnsupportedVersion {
        /// The version the envelope carried.
        offered: u16,
    },
    /// The source and the target are the same session.
    #[error("a session cannot send to itself")]
    SelfSend,
    /// The source is not a live managed session.
    #[error("source {0} is not a live managed session")]
    UnknownSource(ManagedSessionId),
    /// The target is not a live managed session.
    #[error("target {0} is not a live managed session")]
    UnknownTarget(ManagedSessionId),
    /// The envelope names a room, and rooms are not open.
    #[error("rooms are not open")]
    RoomsClosed,
    /// The body is larger than the courier's own ceiling, however it was constructed.
    #[error("a body of {len} bytes exceeds the courier ceiling of {ceiling} bytes")]
    BodyTooLarge {
        /// The body size offered.
        len: usize,
        /// The courier's body ceiling.
        ceiling: usize,
    },
    /// A tell or an ask names a call already open. A fresh message opens a fresh call.
    #[error("call {0} is already open")]
    CallInUse(CallId),
    /// The message identifier was seen before.
    #[error("message {0} was already sent")]
    DuplicateMessage(MessageId),
    /// A tell or an ask names a message it answers.
    #[error("a {0:?} answers nothing and cannot carry reply_to")]
    UnexpectedReplyTo(CallKind),
    /// A reply or a cancel does not name the message it answers.
    #[error("a {0:?} must name the message it answers in reply_to")]
    MissingReplyTo(CallKind),
    /// The deadline is not in the future.
    #[error("deadline {deadline} is not after now {now}")]
    DeadlinePassed {
        /// The deadline offered.
        deadline: UnixMillis,
        /// The moment of sending.
        now: UnixMillis,
    },
    /// The deadline lies further ahead than the ceiling allows.
    #[error("a deadline {millis} ms ahead exceeds the ceiling of {ceiling} ms")]
    DeadlineTooFar {
        /// How far ahead the deadline was placed.
        millis: u64,
        /// The ceiling.
        ceiling: u64,
    },
    /// The message travelled as many hops as the ceiling allows.
    #[error("{hops} hops already travelled reach the ceiling of {ceiling}")]
    HopBound {
        /// Hops travelled before this send.
        hops: u8,
        /// The ceiling.
        ceiling: u8,
    },
    /// The target already saw this message.
    #[error("target {0} was already visited by this message")]
    Cycle(ManagedSessionId),
    /// The visited set is full and the source is not in it.
    #[error(transparent)]
    VisitedBound(#[from] VisitedBound),
    /// As many asks are waiting for a reply as the ceiling allows.
    #[error("{ceiling} calls are already waiting for a reply")]
    TooManyCalls {
        /// The ceiling.
        ceiling: usize,
    },
    /// The target's mailbox holds as many envelopes as the ceiling allows.
    #[error("the mailbox of {session} already holds {ceiling} envelopes")]
    MailboxEnvelopes {
        /// The target.
        session: ManagedSessionId,
        /// The ceiling.
        ceiling: usize,
    },
    /// The target's mailbox cannot take the body without passing its byte ceiling.
    #[error("the mailbox of {session} cannot hold this body under {ceiling} bytes")]
    MailboxBytes {
        /// The target.
        session: ManagedSessionId,
        /// The ceiling.
        ceiling: usize,
    },
    /// Every mailbox together cannot take the body without passing the Runtime ceiling.
    #[error("dialogue bodies cannot hold this body under {ceiling} bytes")]
    RuntimeBytes {
        /// The ceiling.
        ceiling: usize,
    },
    /// The call is not waiting for a reply.
    #[error("call {0} is not waiting for a reply")]
    NoSuchCall(CallId),
    /// The reply or the cancel names a message other than the call's ask.
    #[error("the call's ask is {expected}, not {offered}")]
    WrongMessage {
        /// The ask the call was opened by.
        expected: MessageId,
        /// The message the envelope named.
        offered: MessageId,
    },
    /// The envelope comes from a session the call does not give that role.
    #[error("only {expected} may send this for the call")]
    WrongSource {
        /// The session the call expects.
        expected: ManagedSessionId,
    },
    /// The envelope goes to a session the call does not give that role.
    #[error("this must go to {expected} for the call")]
    WrongTarget {
        /// The session the call expects.
        expected: ManagedSessionId,
    },
    /// The call's deadline passed. The call is gone.
    #[error("call {0} expired")]
    CallExpired(CallId),
    /// The target has not consumed the ask it is replying to.
    #[error("ask {0} has not been received")]
    ReplyBeforeReceipt(MessageId),
    /// The call already has its one reply.
    #[error("call {0} was already replied to")]
    AlreadyReplied(CallId),
}
