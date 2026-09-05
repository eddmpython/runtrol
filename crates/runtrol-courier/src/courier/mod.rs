//! The exchange: the live sessions, their mailboxes, the calls in flight, and the bytes charged for all of it.

use std::collections::{BTreeMap, VecDeque};

use crate::envelope::{BoundedSessionSet, CallEnvelope, CallKind, PROTOCOL_VERSION, VisitedBound};
use crate::id::{CallId, ManagedSessionId, MessageId};
use crate::limits::{Limits, UnixMillis};
use crate::receipt::{DeliveryState, Receipt, Refusal};

mod commands;
pub(crate) mod rooms;

#[cfg(test)]
mod tests;

/// One ask waiting for its reply.
#[derive(Clone, Debug)]
struct ActiveCall {
    ask: MessageId,
    source: ManagedSessionId,
    target: ManagedSessionId,
    deadline: UnixMillis,
    hop_count: u8,
    visited: BoundedSessionSet,
    stage: CallStage,
    room: Option<crate::RoomId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallStage {
    /// The ask sits in the target's mailbox.
    Queued,
    /// The target consumed the ask and owes a reply.
    Received,
    /// The reply sits in the source's mailbox.
    Replied,
}

/// One live session's mail, oldest first.
#[derive(Debug, Default)]
struct Mailbox {
    queue: VecDeque<CallEnvelope>,
    bytes: usize,
}

/// Room a send may count on because the same send withdraws something first.
#[derive(Clone, Copy, Debug, Default)]
struct Freed {
    envelopes: usize,
    bytes: usize,
}

/// What ending a session released.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Released {
    /// Envelopes dropped: the session's own mail, and asks it had queued elsewhere.
    pub envelopes: usize,
    /// Calls ended because the session was their source or their target.
    pub calls: usize,
    /// Body bytes no longer charged.
    pub bytes: usize,
}

/// What one sweep of the clock expired.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Messages dropped from mailboxes because their deadline passed unread.
    pub messages: Vec<MessageId>,
    /// Calls ended because their deadline passed before their reply was read.
    pub calls: Vec<CallId>,
    /// Body bytes no longer charged.
    pub bytes: usize,
}

/// The courier's whole state. It owns no thread, no clock, and no socket.
#[derive(Debug)]
pub struct Courier {
    limits: Limits,
    mailboxes: BTreeMap<ManagedSessionId, Mailbox>,
    calls: BTreeMap<CallId, ActiveCall>,
    rooms: BTreeMap<crate::RoomId, rooms::Room>,
    /// Every message identifier known, live or retired, with where it got to.
    states: BTreeMap<MessageId, DeliveryState>,
    /// The retired identifiers, oldest first, bounded by the limits. Live ones are never here.
    retired: VecDeque<MessageId>,
    charged_bytes: usize,
}

impl Courier {
    /// A courier with no live session, enforcing `limits`.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            mailboxes: BTreeMap::new(),
            calls: BTreeMap::new(),
            rooms: BTreeMap::new(),
            states: BTreeMap::new(),
            retired: VecDeque::new(),
            charged_bytes: 0,
        }
    }

    /// The ceilings this courier enforces.
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Body bytes sitting in mailboxes right now.
    #[must_use]
    pub const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    /// Asks waiting for their reply right now.
    #[must_use]
    pub fn active_calls(&self) -> usize {
        self.calls.len()
    }

    /// Whether `session` may send and be sent to.
    #[must_use]
    pub fn is_live(&self, session: ManagedSessionId) -> bool {
        self.mailboxes.contains_key(&session)
    }

    /// Envelopes waiting for `session`, or `None` when it is not live.
    #[must_use]
    pub fn waiting(&self, session: ManagedSessionId) -> Option<usize> {
        self.mailboxes
            .get(&session)
            .map(|mailbox| mailbox.queue.len())
    }

    /// Where a message this courier still remembers got to.
    #[must_use]
    pub fn state_of(&self, message: MessageId) -> Option<DeliveryState> {
        self.states.get(&message).copied()
    }

    /// Admit `session` as a live source and target. `false` when it was live already.
    pub fn session_started(&mut self, session: ManagedSessionId) -> bool {
        if self.mailboxes.contains_key(&session) {
            return false;
        }
        self.mailboxes.insert(session, Mailbox::default());
        true
    }

    /// Forget `session`: its mail is dropped, the calls it took part in end, and their bytes are released now.
    ///
    /// An ordinary reply already sent stays readable by its asker. A room retires when any participant ends,
    /// including its unread replies; unrelated mail keeps its ordinary lifetime.
    pub fn session_ended(&mut self, session: ManagedSessionId) -> Released {
        let mut released = self.end_rooms_of(session);
        let Some(mailbox) = self.mailboxes.remove(&session) else {
            return released;
        };
        self.charged_bytes = self.charged_bytes.saturating_sub(mailbox.bytes);
        for envelope in mailbox.queue {
            released.envelopes = released.envelopes.saturating_add(1);
            released.bytes = released.bytes.saturating_add(envelope.body.len());
            self.retire(envelope.message_id, DeliveryState::Cancelled);
        }

        let ended: Vec<CallId> = self
            .calls
            .iter()
            .filter(|(_, call)| call.source == session || call.target == session)
            .map(|(id, _call)| *id)
            .collect();
        for id in ended {
            let Some(call) = self.calls.remove(&id) else {
                continue;
            };
            released.calls = released.calls.saturating_add(1);
            if call.stage == CallStage::Queued
                && call.source == session
                && let Some(bytes) = self.withdraw(call.target, call.ask)
            {
                released.envelopes = released.envelopes.saturating_add(1);
                released.bytes = released.bytes.saturating_add(bytes);
            }
            let outcome = if call.stage == CallStage::Replied {
                DeliveryState::Replied
            } else {
                DeliveryState::Cancelled
            };
            self.retire(call.ask, outcome);
        }
        released
    }

    /// Admit one envelope, or refuse it and leave everything as it was.
    ///
    /// # Errors
    ///
    /// The refusal names the first check the envelope failed, in this order: the protocol version, a self-send,
    /// a live source, a live target, a room, a duplicate message identifier, the shape its kind requires, the
    /// deadline, the body ceiling; then for an ask the call ceiling; for a tell or an ask a call already open, the
    /// hop bound, a cycle, the visit bound and the room in the target's mailbox; for a reply or a cancel the call it
    /// names, the roles that call gives, and the room in the target's mailbox.
    pub fn send(&mut self, envelope: CallEnvelope, now: UnixMillis) -> Result<Receipt, Refusal> {
        self.admit_shape(&envelope, now)?;
        match envelope.kind {
            CallKind::Tell => self.route(envelope, false),
            CallKind::Ask => {
                if self.calls.len() >= self.limits.active_calls {
                    return Err(Refusal::TooManyCalls {
                        ceiling: self.limits.active_calls,
                    });
                }
                self.route(envelope, true)
            }
            CallKind::Reply => self.reply(envelope, now),
            CallKind::Cancel => self.cancel(envelope, now),
        }
    }

    /// Hand `session` its oldest unexpired envelope, once. Mail that expired while waiting is dropped first.
    pub fn receive(&mut self, session: ManagedSessionId, now: UnixMillis) -> Option<CallEnvelope> {
        self.receive_matching(session, None, None, now)
    }

    /// Consume the oldest matching envelope. A call filter receives only that call's reply; unrelated mail
    /// stays in its original order and keeps its byte charge. The caller must authorize the session first.
    pub fn receive_matching(
        &mut self,
        session: ManagedSessionId,
        source: Option<ManagedSessionId>,
        call: Option<crate::CallRef>,
        now: UnixMillis,
    ) -> Option<CallEnvelope> {
        let mut swept = Swept::default();
        self.expire_rooms(now, &mut swept);
        self.expire_waiting(session, now, &mut swept);
        let mailbox = self.mailboxes.get_mut(&session)?;
        let index = mailbox.queue.iter().position(|envelope| {
            source.is_none_or(|source| envelope.source == source)
                && call.is_none_or(|call| {
                    envelope.call_id == call.call_id
                        && envelope.reply_to == Some(call.ask)
                        && envelope.kind == CallKind::Reply
                })
        })?;
        let envelope = mailbox.queue.remove(index)?;
        mailbox.bytes = mailbox.bytes.saturating_sub(envelope.body.len());
        self.charged_bytes = self.charged_bytes.saturating_sub(envelope.body.len());
        match envelope.kind {
            CallKind::Tell | CallKind::Cancel => {
                self.retire(envelope.message_id, DeliveryState::Received);
            }
            CallKind::Ask => match self.calls.get_mut(&envelope.call_id) {
                Some(call) if call.ask == envelope.message_id => {
                    call.stage = CallStage::Received;
                    self.states
                        .insert(envelope.message_id, DeliveryState::Received);
                }
                // A stray ask whose call is gone or names another message is just delivered and forgotten.
                _ => self.retire(envelope.message_id, DeliveryState::Received),
            },
            CallKind::Reply => {
                if self.matches_call(&envelope) {
                    self.calls.remove(&envelope.call_id);
                }
                self.retire(envelope.message_id, DeliveryState::Received);
                if let Some(ask) = envelope.reply_to {
                    self.retire(ask, DeliveryState::Replied);
                }
            }
        }
        Some(envelope)
    }

    /// Expire every envelope and every call whose deadline is at or before `now`, releasing their bytes.
    pub fn sweep(&mut self, now: UnixMillis) -> Swept {
        let mut swept = Swept::default();
        self.expire_rooms(now, &mut swept);
        let sessions: Vec<ManagedSessionId> = self.mailboxes.keys().copied().collect();
        for session in sessions {
            self.expire_waiting(session, now, &mut swept);
        }
        let expired: Vec<CallId> = self
            .calls
            .iter()
            .filter(|(_, call)| call.deadline <= now)
            .map(|(id, _call)| *id)
            .collect();
        for id in expired {
            let Some(call) = self.calls.remove(&id) else {
                continue;
            };
            self.retire(call.ask, DeliveryState::Expired);
            swept.calls.push(id);
        }
        swept
    }

    fn admit_shape(&self, envelope: &CallEnvelope, now: UnixMillis) -> Result<(), Refusal> {
        if envelope.protocol_version != PROTOCOL_VERSION {
            return Err(Refusal::UnsupportedVersion {
                offered: envelope.protocol_version,
            });
        }
        if envelope.source == envelope.target {
            return Err(Refusal::SelfSend);
        }
        if !self.is_live(envelope.source) {
            return Err(Refusal::UnknownSource(envelope.source));
        }
        if !self.is_live(envelope.target) {
            return Err(Refusal::UnknownTarget(envelope.target));
        }
        if envelope.room_id.is_some() {
            return Err(Refusal::RoomsClosed);
        }
        if self.states.contains_key(&envelope.message_id) {
            return Err(Refusal::DuplicateMessage(envelope.message_id));
        }
        match (envelope.kind, envelope.reply_to) {
            (CallKind::Tell | CallKind::Ask, Some(_)) => {
                return Err(Refusal::UnexpectedReplyTo(envelope.kind));
            }
            (CallKind::Reply | CallKind::Cancel, None) => {
                return Err(Refusal::MissingReplyTo(envelope.kind));
            }
            _ => {}
        }
        if envelope.deadline <= now {
            return Err(Refusal::DeadlinePassed {
                deadline: envelope.deadline,
                now,
            });
        }
        let ahead = envelope.deadline.since(now);
        if ahead > self.limits.max_deadline_millis {
            return Err(Refusal::DeadlineTooFar {
                millis: ahead,
                ceiling: self.limits.max_deadline_millis,
            });
        }
        // The ceiling is the courier's, not the caller's: a body admitted through a `BoundedUtf8` built with a
        // larger ceiling is still refused here, so this one place owns the limit the design fixed.
        if envelope.body.len() > self.limits.body_bytes {
            return Err(Refusal::BodyTooLarge {
                len: envelope.body.len(),
                ceiling: self.limits.body_bytes,
            });
        }
        Ok(())
    }

    /// A tell or an ask: check the chain, find room, stamp the delivery, and queue it.
    fn route(&mut self, envelope: CallEnvelope, opens_call: bool) -> Result<Receipt, Refusal> {
        // A fresh message opens a fresh call. Naming a call already open would overwrite its record, so a
        // caller could hide many asks behind one call and slip past the active-call ceiling.
        if self.calls.contains_key(&envelope.call_id) {
            return Err(Refusal::CallInUse(envelope.call_id));
        }
        if envelope.hop_count >= self.limits.hop_count {
            return Err(Refusal::HopBound {
                hops: envelope.hop_count,
                ceiling: self.limits.hop_count,
            });
        }
        if envelope.visited.contains(envelope.target) {
            return Err(Refusal::Cycle(envelope.target));
        }
        if envelope.visited.len() > self.limits.visited_sessions {
            return Err(Refusal::VisitedBound(VisitedBound {
                len: envelope.visited.len(),
                ceiling: self.limits.visited_sessions,
            }));
        }
        let visited = envelope
            .visited
            .with(envelope.source, self.limits.visited_sessions)?;
        self.room_for(envelope.target, envelope.body.len(), Freed::default())?;
        let delivered = CallEnvelope {
            hop_count: envelope.hop_count.saturating_add(1),
            visited,
            ..envelope
        };
        if opens_call {
            self.calls.insert(
                delivered.call_id,
                ActiveCall {
                    ask: delivered.message_id,
                    source: delivered.source,
                    target: delivered.target,
                    deadline: delivered.deadline,
                    hop_count: delivered.hop_count,
                    visited: delivered.visited.clone(),
                    stage: CallStage::Queued,
                    room: delivered.room_id,
                },
            );
        }
        Ok(self.enqueue(delivered))
    }

    /// The one reply to an open call: only from the call's target, only to its source, only once, only in time.
    fn reply(&mut self, envelope: CallEnvelope, now: UnixMillis) -> Result<Receipt, Refusal> {
        let answers = envelope
            .reply_to
            .ok_or(Refusal::MissingReplyTo(CallKind::Reply))?;
        let call = self.call_named(&envelope, answers)?;
        if call.target != envelope.source {
            return Err(Refusal::WrongSource {
                expected: call.target,
            });
        }
        if call.source != envelope.target {
            return Err(Refusal::WrongTarget {
                expected: call.source,
            });
        }
        if call.deadline <= now {
            // A refusal changes nothing: the call is left for the sweep, which expires it and its ask together.
            return Err(Refusal::CallExpired(envelope.call_id));
        }
        match call.stage {
            CallStage::Queued => return Err(Refusal::ReplyBeforeReceipt(call.ask)),
            CallStage::Replied => return Err(Refusal::AlreadyReplied(envelope.call_id)),
            CallStage::Received => {}
        }
        self.room_for(envelope.target, envelope.body.len(), Freed::default())?;
        // A reply cannot outlive its call or reset the chain the target received.
        let delivered = CallEnvelope {
            deadline: envelope.deadline.min(call.deadline),
            room_id: call.room,
            hop_count: call.hop_count,
            visited: call.visited,
            ..envelope
        };
        if let Some(open) = self.calls.get_mut(&delivered.call_id) {
            open.stage = CallStage::Replied;
        }
        self.states.insert(call.ask, DeliveryState::Replied);
        Ok(self.enqueue(delivered))
    }

    /// The asker withdrawing its ask: an unread ask leaves the target's mailbox, the call ends, and the target is
    /// told either way.
    fn cancel(&mut self, envelope: CallEnvelope, now: UnixMillis) -> Result<Receipt, Refusal> {
        let answers = envelope
            .reply_to
            .ok_or(Refusal::MissingReplyTo(CallKind::Cancel))?;
        let call = self.call_named(&envelope, answers)?;
        if call.deadline <= now {
            return Err(Refusal::CallExpired(envelope.call_id));
        }
        if call.source != envelope.source {
            return Err(Refusal::WrongSource {
                expected: call.source,
            });
        }
        if call.target != envelope.target {
            return Err(Refusal::WrongTarget {
                expected: call.target,
            });
        }
        if call.stage == CallStage::Replied {
            return Err(Refusal::AlreadyReplied(envelope.call_id));
        }
        let freed = if call.stage == CallStage::Queued {
            Freed {
                envelopes: 1,
                bytes: self.queued_bytes(call.target, call.ask),
            }
        } else {
            Freed::default()
        };
        self.room_for(envelope.target, envelope.body.len(), freed)?;
        if call.stage == CallStage::Queued {
            self.withdraw(call.target, call.ask);
        }
        self.calls.remove(&envelope.call_id);
        self.retire(call.ask, DeliveryState::Cancelled);
        Ok(self.enqueue(CallEnvelope {
            room_id: call.room,
            hop_count: call.hop_count,
            visited: call.visited,
            ..envelope
        }))
    }

    fn call_named(
        &self,
        envelope: &CallEnvelope,
        answers: MessageId,
    ) -> Result<ActiveCall, Refusal> {
        let call = self
            .calls
            .get(&envelope.call_id)
            .ok_or(Refusal::NoSuchCall(envelope.call_id))?;
        if call.ask != answers {
            return Err(Refusal::WrongMessage {
                expected: call.ask,
                offered: answers,
            });
        }
        Ok(call.clone())
    }

    fn matches_call(&self, envelope: &CallEnvelope) -> bool {
        self.calls
            .get(&envelope.call_id)
            .is_some_and(|call| match envelope.kind {
                CallKind::Ask => {
                    call.ask == envelope.message_id
                        && call.source == envelope.source
                        && call.target == envelope.target
                }
                CallKind::Reply => {
                    envelope.reply_to == Some(call.ask)
                        && call.target == envelope.source
                        && call.source == envelope.target
                }
                CallKind::Tell | CallKind::Cancel => false,
            })
    }

    fn room_for(
        &self,
        target: ManagedSessionId,
        body_len: usize,
        freed: Freed,
    ) -> Result<(), Refusal> {
        let mailbox = self
            .mailboxes
            .get(&target)
            .ok_or(Refusal::UnknownTarget(target))?;
        let envelopes = mailbox.queue.len().saturating_sub(freed.envelopes);
        if envelopes >= self.limits.mailbox_envelopes {
            return Err(Refusal::MailboxEnvelopes {
                session: target,
                ceiling: self.limits.mailbox_envelopes,
            });
        }
        let mailbox_bytes = mailbox.bytes.saturating_sub(freed.bytes);
        if mailbox_bytes.saturating_add(body_len) > self.limits.mailbox_bytes {
            return Err(Refusal::MailboxBytes {
                session: target,
                ceiling: self.limits.mailbox_bytes,
            });
        }
        let runtime_bytes = self.charged_bytes.saturating_sub(freed.bytes);
        if runtime_bytes.saturating_add(body_len) > self.limits.runtime_bytes {
            return Err(Refusal::RuntimeBytes {
                ceiling: self.limits.runtime_bytes,
            });
        }
        Ok(())
    }

    /// Queue an admitted envelope and charge its body. Every check has passed by now.
    fn enqueue(&mut self, envelope: CallEnvelope) -> Receipt {
        let receipt = Receipt {
            message_id: envelope.message_id,
            call_id: envelope.call_id,
            state: DeliveryState::Accepted,
        };
        let len = envelope.body.len();
        self.states
            .insert(envelope.message_id, DeliveryState::Accepted);
        if let Some(mailbox) = self.mailboxes.get_mut(&envelope.target) {
            mailbox.bytes = mailbox.bytes.saturating_add(len);
            mailbox.queue.push_back(envelope);
            self.charged_bytes = self.charged_bytes.saturating_add(len);
        }
        receipt
    }

    fn queued_bytes(&self, session: ManagedSessionId, message: MessageId) -> usize {
        self.mailboxes
            .get(&session)
            .and_then(|mailbox| {
                mailbox
                    .queue
                    .iter()
                    .find(|envelope| envelope.message_id == message)
            })
            .map_or(0, |envelope| envelope.body.len())
    }

    /// Take `message` out of `session`'s mailbox unread, releasing its bytes. Its state is left to the caller.
    fn withdraw(&mut self, session: ManagedSessionId, message: MessageId) -> Option<usize> {
        let mailbox = self.mailboxes.get_mut(&session)?;
        let index = mailbox
            .queue
            .iter()
            .position(|envelope| envelope.message_id == message)?;
        let envelope = mailbox.queue.remove(index)?;
        let len = envelope.body.len();
        mailbox.bytes = mailbox.bytes.saturating_sub(len);
        self.charged_bytes = self.charged_bytes.saturating_sub(len);
        Some(len)
    }

    /// Drop `session`'s mail whose deadline is at or before `now`, ending the calls that mail carried.
    fn expire_waiting(&mut self, session: ManagedSessionId, now: UnixMillis, swept: &mut Swept) {
        let expired = {
            let Some(mailbox) = self.mailboxes.get_mut(&session) else {
                return;
            };
            let (expired, kept): (Vec<CallEnvelope>, Vec<CallEnvelope>) = mailbox
                .queue
                .drain(..)
                .partition(|envelope| envelope.deadline <= now);
            mailbox.queue.extend(kept);
            let freed: usize = expired.iter().map(|envelope| envelope.body.len()).sum();
            mailbox.bytes = mailbox.bytes.saturating_sub(freed);
            expired
        };
        for envelope in expired {
            let len = envelope.body.len();
            self.charged_bytes = self.charged_bytes.saturating_sub(len);
            swept.bytes = swept.bytes.saturating_add(len);
            swept.messages.push(envelope.message_id);
            self.retire(envelope.message_id, DeliveryState::Expired);
            match envelope.kind {
                CallKind::Ask | CallKind::Reply => {
                    if self.matches_call(&envelope)
                        && self.calls.remove(&envelope.call_id).is_some()
                    {
                        swept.calls.push(envelope.call_id);
                    }
                    if let Some(ask) = envelope.reply_to {
                        // The reply arrived and was never read: the ask was answered, the answer was lost.
                        self.retire(ask, DeliveryState::Replied);
                    }
                }
                CallKind::Tell | CallKind::Cancel => {}
            }
        }
    }

    /// Record `message`'s final state and remember it, forgetting the oldest retired identifier past the ceiling.
    fn retire(&mut self, message: MessageId, state: DeliveryState) {
        if self.retired.contains(&message) {
            return;
        }
        self.states.insert(message, state);
        self.retired.push_back(message);
        while self.retired.len() > self.limits.remembered_messages {
            if let Some(forgotten) = self.retired.pop_front() {
                self.states.remove(&forgotten);
            }
        }
    }
}
