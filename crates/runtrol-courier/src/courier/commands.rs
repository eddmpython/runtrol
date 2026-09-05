//! Command correlation uses existing call metadata, never a retained request body.

use super::{CallStage, Courier};
use crate::{
    BoundedUtf8, CallEnvelope, CallId, CallKind, ManagedSessionId, MessageId, Receipt, Refusal,
    UnixMillis,
};

impl Courier {
    /// The next body or call expiry. A transport can sleep until this instant without polling idle state.
    #[must_use]
    pub fn next_deadline(&self) -> Option<UnixMillis> {
        self.mailboxes
            .values()
            .flat_map(|mailbox| mailbox.queue.iter().map(|mail| mail.deadline))
            .chain(self.calls.values().map(|call| call.deadline))
            .chain(self.rooms.values().map(|room| room.deadline))
            .min()
    }

    /// Whether this session owns a call still awaiting receipt of its answer.
    #[must_use]
    pub fn owns_call(&self, session: ManagedSessionId, call: crate::CallRef) -> bool {
        self.calls
            .get(&call.call_id)
            .is_some_and(|active| active.source == session && active.ask == call.ask)
    }

    /// Reply to the exact received ask, deriving roles, deadline, and chain from the call authority.
    ///
    /// # Errors
    /// Refuses an unknown ask, wrong session, unread ask, duplicate, expired call, or exceeded limit.
    pub fn answer(
        &mut self,
        session: ManagedSessionId,
        message: MessageId,
        message_id: MessageId,
        body: BoundedUtf8,
        now: UnixMillis,
    ) -> Result<Receipt, Refusal> {
        let (id, source, deadline) = self
            .calls
            .iter()
            .find(|(_, call)| call.ask == message && call.target == session)
            .map(|(id, call)| (*id, call.source, call.deadline))
            .ok_or(Refusal::ReplyBeforeReceipt(message))?;
        let mut envelope = CallEnvelope::tell(session, source, body, deadline);
        envelope.message_id = message_id;
        envelope.call_id = id;
        envelope.kind = CallKind::Reply;
        envelope.reply_to = Some(message);
        self.send(envelope, now)
    }

    /// Withdraw a call as its authenticated asker and notify its target.
    ///
    /// # Errors
    /// Refuses unknown calls, another asker's call, an answered call, or exceeded limits. A received ask
    /// stays open when its target has no room for the cancellation notice; a refusal changes nothing.
    pub fn cancel_call(
        &mut self,
        session: ManagedSessionId,
        call_id: CallId,
        message_id: MessageId,
        now: UnixMillis,
    ) -> Result<Receipt, Refusal> {
        let (target, ask, deadline) = self
            .calls
            .get(&call_id)
            .filter(|call| call.source == session)
            .map(|call| (call.target, call.ask, call.deadline))
            .ok_or(Refusal::NoSuchCall(call_id))?;
        let mut envelope = CallEnvelope::tell(session, target, BoundedUtf8::empty(), deadline);
        envelope.message_id = message_id;
        envelope.call_id = call_id;
        envelope.kind = CallKind::Cancel;
        envelope.reply_to = Some(ask);
        self.send(envelope, now)
    }

    /// Release the exact ask whose waiting client disconnected, without queuing a cancellation notice.
    /// A full target mailbox cannot prevent this cleanup. A completed reply stays readable. Callers sweep
    /// expiry first so a deadline that passed is recorded as expired rather than cancelled.
    pub fn abandon_call(&mut self, session: ManagedSessionId, call: crate::CallRef) {
        let call_id = call.call_id;
        let Some(active) = self
            .calls
            .get(&call_id)
            .filter(|active| active.source == session && active.ask == call.ask)
        else {
            return;
        };
        if active.stage == CallStage::Replied {
            // A completed answer remains available to a later explicit inbox read.
            return;
        }
        let Some(call) = self.calls.remove(&call_id) else {
            return;
        };
        if call.stage == CallStage::Queued {
            self.withdraw(call.target, call.ask);
        }
        self.retire(call.ask, crate::DeliveryState::Cancelled);
    }
}
