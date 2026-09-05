//! One authenticated command and one bounded answer per connection.

use serde::{Deserialize, Serialize};

use crate::{BoundedUtf8, CallEnvelope, CallId, ManagedSessionId, MessageId, Receipt};

/// Maximum structural rows in one listing. Continue after its last identifier to fetch the next page.
pub const SESSION_PAGE: usize = 32;
/// Admitted commands held simultaneously, independent of the short greeting slots.
pub const COMMAND_SLOTS: usize = 32;
/// Long waits have a separate allowance so they cannot exclude messages that would wake them.
pub const WAIT_SLOTS: usize = crate::Limits::INITIAL.active_calls;
/// One session cannot occupy every Runtime wait slot.
pub const SESSION_WAIT_SLOTS: usize = 4;

/// A hello optionally carrying one command. A hello alone remains the admission probe.
#[derive(Debug, Serialize, Deserialize)]
pub struct Invocation {
    /// The existing process-scoped admission proof.
    #[serde(flatten)]
    pub hello: super::Hello,
    /// Executed only after the hello is admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Request>,
}

/// Commands carry explicit structural identifiers and opaque bodies, never provider instructions.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Live managed session identities in ascending order.
    List {
        /// Exclusive cursor from a previous page.
        after: Option<ManagedSessionId>,
    },
    /// Route an explicit envelope. Its source must match the admitted session.
    Send {
        /// Caller-created routing metadata and opaque body.
        envelope: CallEnvelope,
    },
    /// Admit an ask and await its exact reply on the same connection. Closing it releases the pending call.
    Ask {
        /// An ask whose source is the admitted session.
        envelope: CallEnvelope,
    },
    /// Consume one matching message, waiting at most the requested milliseconds.
    Receive {
        /// Match this sender without removing unrelated mail.
        source: Option<ManagedSessionId>,
        /// Receive only the reply to this exact ask.
        call: Option<crate::CallRef>,
        /// Zero consumes immediately; positive values are capped by the courier limits.
        timeout_ms: u64,
    },
    /// Answer an exact received ask. The Runtime derives the call and peer from its metadata.
    Reply {
        /// The exact received ask being answered.
        message: MessageId,
        /// This outgoing envelope's duplicate identifier.
        message_id: MessageId,
        /// Caller-supplied opaque reply.
        body: BoundedUtf8,
    },
    /// Withdraw the admitted session's call.
    Cancel {
        /// The admitted session's call to withdraw.
        call: CallId,
        /// This cancellation's duplicate identifier.
        message_id: MessageId,
    },
}

/// A process identity established by this Runtime, with no conversation content.
#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    /// Process-local managed session identifier.
    pub session: ManagedSessionId,
    /// The session root's OS process identifier.
    pub pid: u32,
}

/// Command outcomes. Receipt means transport admission, never model understanding or task completion.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    /// One page. `next` is the cursor for another page, if one exists.
    Sessions {
        /// Bounded live structural rows.
        sessions: Vec<Session>,
        /// Exclusive cursor, absent at the end of the listing.
        next: Option<ManagedSessionId>,
    },
    /// The envelope was admitted exactly once.
    Accepted {
        /// Exact identifiers and mechanical admission state.
        receipt: Receipt,
    },
    /// One consumed envelope, or no message before the bounded wait ended.
    Received {
        /// Consumed mail, absent when the wait found none.
        envelope: Option<CallEnvelope>,
    },
    /// Mechanical refusal without a body or secret.
    Refused {
        /// Structural failure only, with neither body nor admission token.
        reason: String,
    },
}
