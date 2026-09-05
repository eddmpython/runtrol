//! The envelope a caller writes and the courier routes.

use crate::body::BoundedUtf8;
use crate::id::{CallId, ManagedSessionId, MessageId, RoomId};
use crate::limits::UnixMillis;

/// The envelope layout this crate speaks. An envelope of any other version is refused before it is read further.
pub const PROTOCOL_VERSION: u16 = 1;

/// The routing intent a caller declares. It is never inferred from the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// One-way. Delivery is the whole contract.
    Tell,
    /// A request that waits for exactly one correlated reply before its deadline.
    Ask,
    /// The one answer to an ask, travelling back under the ask's call.
    Reply,
    /// The asker withdrawing its ask.
    Cancel,
}

/// The sessions a message has already passed through, oldest first, without repeats and under a ceiling.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct BoundedSessionSet {
    sessions: Vec<ManagedSessionId>,
}

impl<'de> serde::Deserialize<'de> for BoundedSessionSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let sessions = Vec::<ManagedSessionId>::deserialize(deserializer)?;
        let mut visited = Self::new();
        for session in sessions {
            visited = visited
                .with(session, crate::Limits::INITIAL.visited_sessions)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(visited)
    }
}

/// More visits than the ceiling allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{len} visited sessions exceed the ceiling of {ceiling}")]
pub struct VisitedBound {
    /// How many sessions were visited already.
    pub len: usize,
    /// The ceiling that was reached.
    pub ceiling: usize,
}

impl BoundedSessionSet {
    /// Nobody visited yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sessions: Vec::new(),
        }
    }

    /// Whether `session` was visited.
    #[must_use]
    pub fn contains(&self, session: ManagedSessionId) -> bool {
        self.sessions.contains(&session)
    }

    /// How many sessions were visited.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether nobody was visited.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// The visited sessions, oldest first.
    #[must_use]
    pub fn as_slice(&self) -> &[ManagedSessionId] {
        &self.sessions
    }

    /// This set with `session` appended. A session already present is not appended again.
    ///
    /// # Errors
    ///
    /// Appending a new session to a set that already holds `ceiling` sessions is refused.
    pub fn with(&self, session: ManagedSessionId, ceiling: usize) -> Result<Self, VisitedBound> {
        if self.contains(session) {
            return Ok(self.clone());
        }
        if self.sessions.len() >= ceiling {
            return Err(VisitedBound {
                len: self.sessions.len(),
                ceiling,
            });
        }
        let mut sessions = Vec::with_capacity(self.sessions.len().saturating_add(1));
        sessions.extend_from_slice(&self.sessions);
        sessions.push(session);
        Ok(Self { sessions })
    }
}

/// One message from one managed session to another.
///
/// The fields are the wire layout. A caller builds an envelope through the constructors below, which is also
/// what the courier command does; the courier validates every field on admission and stamps the hop count and
/// the visited set on delivery, so a message that continues a chain carries the chain with it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallEnvelope {
    /// The layout version. Must equal [`PROTOCOL_VERSION`].
    pub protocol_version: u16,
    /// This envelope. Idempotent: the same identifier is never delivered twice.
    pub message_id: MessageId,
    /// The call this envelope belongs to. A tell or an ask opens one; a reply or a cancel names an open one.
    pub call_id: CallId,
    /// The session sending.
    pub source: ManagedSessionId,
    /// The session receiving.
    pub target: ManagedSessionId,
    /// The declared routing intent.
    pub kind: CallKind,
    /// The exact message a reply or a cancel answers. Absent on a tell and an ask.
    pub reply_to: Option<MessageId>,
    /// The room this envelope speaks in. Rooms open in a later stamp; until then any room is refused.
    pub room_id: Option<RoomId>,
    /// The moment after which this envelope is worthless and its body is released.
    pub deadline: UnixMillis,
    /// Hops travelled so far. Zero on a fresh message; the courier adds one when it routes a tell or an ask.
    /// A reply and a cancel carry the call's own count unchanged, because the call's deadline and its one
    /// reply bound them rather than the hop ceiling.
    pub hop_count: u8,
    /// Sessions this message passed through so far. The courier adds the source when it routes a tell or an
    /// ask; a reply and a cancel carry the call's set unchanged.
    pub visited: BoundedSessionSet,
    /// The opaque body.
    pub body: BoundedUtf8,
}

impl CallEnvelope {
    fn fresh(
        kind: CallKind,
        source: ManagedSessionId,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: MessageId::now(),
            call_id: CallId::now(),
            source,
            target,
            kind,
            reply_to: None,
            room_id: None,
            deadline,
            hop_count: 0,
            visited: BoundedSessionSet::new(),
            body,
        }
    }

    /// A fresh one-way message. It starts its own chain: no hops, nobody visited.
    #[must_use]
    pub fn tell(
        source: ManagedSessionId,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self::fresh(CallKind::Tell, source, target, body, deadline)
    }

    /// A fresh request that waits for exactly one reply before `deadline`.
    #[must_use]
    pub fn ask(
        source: ManagedSessionId,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self::fresh(CallKind::Ask, source, target, body, deadline)
    }

    /// The one reply to `received`, an ask this session was handed. It travels back to the asker under the
    /// ask's call and carries the chain the ask arrived with.
    #[must_use]
    pub fn reply(received: &Self, body: BoundedUtf8, deadline: UnixMillis) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: MessageId::now(),
            call_id: received.call_id,
            source: received.target,
            target: received.source,
            kind: CallKind::Reply,
            reply_to: Some(received.message_id),
            room_id: None,
            deadline,
            hop_count: received.hop_count,
            visited: received.visited.clone(),
            body,
        }
    }

    /// The withdrawal of `sent`, an ask this session sent. It carries no body.
    #[must_use]
    pub fn cancel(sent: &Self, deadline: UnixMillis) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: MessageId::now(),
            call_id: sent.call_id,
            source: sent.source,
            target: sent.target,
            kind: CallKind::Cancel,
            reply_to: Some(sent.message_id),
            room_id: None,
            deadline,
            hop_count: sent.hop_count,
            visited: sent.visited.clone(),
            body: BoundedUtf8::empty(),
        }
    }

    /// A one-way message that continues `received`: it carries the hops and the visits so far, so the courier
    /// refuses it once the chain turns back on a visited session or runs past the hop ceiling.
    #[must_use]
    pub fn forward(
        received: &Self,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self::onward(received, CallKind::Tell, target, body, deadline)
    }

    /// A request that continues `received`, under the same chain rules as [`Self::forward`].
    #[must_use]
    pub fn ask_onward(
        received: &Self,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self::onward(received, CallKind::Ask, target, body, deadline)
    }

    fn onward(
        received: &Self,
        kind: CallKind,
        target: ManagedSessionId,
        body: BoundedUtf8,
        deadline: UnixMillis,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: MessageId::now(),
            call_id: CallId::now(),
            source: received.target,
            target,
            kind,
            reply_to: None,
            room_id: None,
            deadline,
            hop_count: received.hop_count,
            visited: received.visited.clone(),
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visit_set_ignores_a_repeat_and_refuses_growth_past_the_ceiling() {
        let first = ManagedSessionId::now();
        let second = ManagedSessionId::now();
        let third = ManagedSessionId::now();
        let visited = BoundedSessionSet::new()
            .with(first, 2)
            .expect("one under two")
            .with(second, 2)
            .expect("two under two");
        assert_eq!(visited.as_slice(), &[first, second]);
        assert_eq!(
            visited.with(second, 2).expect("a repeat costs nothing"),
            visited
        );
        assert_eq!(
            visited.with(third, 2),
            Err(VisitedBound { len: 2, ceiling: 2 })
        );
        assert!(BoundedSessionSet::default().is_empty());
        assert_eq!(visited.len(), 2);
    }

    #[test]
    fn a_reply_turns_a_received_ask_around_and_a_cancel_carries_nothing() {
        let asker = ManagedSessionId::now();
        let answerer = ManagedSessionId::now();
        let body = BoundedUtf8::new("question".to_owned(), 64).expect("fits");
        let ask = CallEnvelope::ask(asker, answerer, body, UnixMillis(10));
        assert_eq!(ask.hop_count, 0);
        assert!(ask.visited.is_empty());
        assert_eq!(ask.reply_to, None);

        let answer = BoundedUtf8::new("answer".to_owned(), 64).expect("fits");
        let reply = CallEnvelope::reply(&ask, answer, UnixMillis(20));
        assert_eq!(reply.kind, CallKind::Reply);
        assert_eq!(reply.call_id, ask.call_id);
        assert_eq!(reply.reply_to, Some(ask.message_id));
        assert_eq!((reply.source, reply.target), (answerer, asker));
        assert_ne!(reply.message_id, ask.message_id);

        let cancel = CallEnvelope::cancel(&ask, UnixMillis(20));
        assert_eq!(cancel.kind, CallKind::Cancel);
        assert_eq!((cancel.source, cancel.target), (asker, answerer));
        assert_eq!(cancel.reply_to, Some(ask.message_id));
        assert!(cancel.body.is_empty());

        let third = ManagedSessionId::now();
        let onward = CallEnvelope::forward(&ask, third, BoundedUtf8::empty(), UnixMillis(20));
        assert_eq!((onward.source, onward.target), (answerer, third));
        assert_eq!(onward.kind, CallKind::Tell);
        assert_ne!(onward.call_id, ask.call_id, "a forward opens its own call");
        assert_eq!(
            CallEnvelope::ask_onward(&ask, third, BoundedUtf8::empty(), UnixMillis(20)).kind,
            CallKind::Ask
        );
    }
}
