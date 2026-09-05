//! Explicit, bounded rounds. Room membership carries no body and starts no participant activity.

use crate::{
    BoundedSessionSet, BoundedUtf8, CallEnvelope, CallRef, DeliveryState, ManagedSessionId,
    MessageId, Receipt, Refusal, RoomId, UnixMillis,
};

use super::{CallStage, Courier, Released, Swept};

mod cleanup;

#[cfg(test)]
mod tests;

/// Structural state of one room, visible only to its participants.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoomView {
    /// This room's identity.
    pub id: RoomId,
    /// The participant authorized to transfer the speaker or close the room.
    pub owner: ManagedSessionId,
    /// The participant authorized to open the next round.
    pub speaker: ManagedSessionId,
    /// Fixed participants, including the owner, bounded by the courier's limits.
    pub participants: BoundedSessionSet,
    /// Admitted rounds, including rounds later cancelled or abandoned.
    pub rounds: u8,
    /// A round stays in flight until its reply is consumed or its call ends.
    pub in_flight: Option<CallRef>,
    /// The room and every round expire by this instant.
    pub deadline: UnixMillis,
}

#[derive(Debug)]
pub(super) struct Room {
    owner: ManagedSessionId,
    speaker: ManagedSessionId,
    participants: BoundedSessionSet,
    rounds: u8,
    current: Option<CallRef>,
    pub(super) deadline: UnixMillis,
}

/// A room operation was refused without changing room or mailbox state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomError {
    /// The identifier names no room in this courier generation.
    #[error("no such room {0}")]
    Unknown(RoomId),
    /// The room deadline passed; a sweep will reclaim it.
    #[error("room {0} expired")]
    Expired(RoomId),
    /// Rooms have the same bounded count as active calls.
    #[error("the room allowance is full")]
    Full,
    /// A room needs at least two participants and no more than its configured ceiling.
    #[error("the room participant count is outside its bound")]
    ParticipantBound,
    /// A fixed participant list may not name a session twice.
    #[error("participant {0} is repeated")]
    DuplicateParticipant(ManagedSessionId),
    /// Every participant must be a live managed session.
    #[error("participant {0} is not live")]
    NotLive(ManagedSessionId),
    /// Only a room participant can inspect it or receive its rounds.
    #[error("session {0} is not a participant")]
    NotParticipant(ManagedSessionId),
    /// Closing the room or transferring its speaker requires its owner.
    #[error("only the room owner may do this")]
    NotOwner,
    /// Only the explicitly selected speaker can open the next round.
    #[error("only the room speaker may ask")]
    NotSpeaker,
    /// A round or its unread reply is still in flight.
    #[error("the previous room round is still in flight")]
    Busy,
    /// A seventh round cannot be admitted under the initial six-round contract.
    #[error("the room round limit is exhausted")]
    RoundBound,
    /// A room deadline must be future and no further than the existing deadline ceiling.
    #[error("the room deadline is outside its bound")]
    Deadline,
    /// Existing envelope, call, or mailbox admission refused the round.
    #[error(transparent)]
    Envelope(#[from] Refusal),
}

impl Courier {
    /// Open a room with fixed live participants and its owner as initial speaker.
    ///
    /// The room count is bounded by `limits.active_calls`. No participant is activated or sent a body.
    ///
    /// # Errors
    /// Refuses invalid membership, an invalid deadline, or the room count ceiling.
    pub fn room_open(
        &mut self,
        owner: ManagedSessionId,
        participants: &[ManagedSessionId],
        deadline: UnixMillis,
        now: UnixMillis,
    ) -> Result<RoomId, RoomError> {
        if participants.len() < 2 || participants.len() > self.limits.room_participants {
            return Err(RoomError::ParticipantBound);
        }
        if !participants.contains(&owner) {
            return Err(RoomError::NotParticipant(owner));
        }
        if deadline <= now || deadline.since(now) > self.limits.max_deadline_millis {
            return Err(RoomError::Deadline);
        }
        if self.rooms.len() >= self.limits.active_calls {
            return Err(RoomError::Full);
        }
        let mut admitted = BoundedSessionSet::new();
        for participant in participants {
            if !self.is_live(*participant) {
                return Err(RoomError::NotLive(*participant));
            }
            if admitted.contains(*participant) {
                return Err(RoomError::DuplicateParticipant(*participant));
            }
            admitted = admitted
                .with(*participant, self.limits.room_participants)
                .map_err(|_full| RoomError::ParticipantBound)?;
        }
        let id = RoomId::now();
        self.rooms.insert(
            id,
            Room {
                owner,
                speaker: owner,
                participants: admitted,
                rounds: 0,
                current: None,
                deadline,
            },
        );
        Ok(id)
    }

    /// Read bounded structural metadata as a participant.
    ///
    /// # Errors
    /// Refuses unknown, expired, or foreign rooms.
    pub fn room_view(
        &self,
        session: ManagedSessionId,
        id: RoomId,
        now: UnixMillis,
    ) -> Result<RoomView, RoomError> {
        let room = self.rooms.get(&id).ok_or(RoomError::Unknown(id))?;
        if !room.participants.contains(session) {
            return Err(RoomError::NotParticipant(session));
        }
        if room.deadline <= now {
            return Err(RoomError::Expired(id));
        }
        Ok(RoomView {
            id,
            owner: room.owner,
            speaker: room.speaker,
            participants: room.participants.clone(),
            rounds: room.rounds,
            in_flight: room.current.filter(|current| {
                self.calls
                    .get(&current.call_id)
                    .is_some_and(|call| call.ask == current.ask && call.room == Some(id))
            }),
            deadline: room.deadline,
        })
    }

    /// Explicitly select the next speaker as the room owner, only between completed rounds.
    ///
    /// # Errors
    /// Refuses a foreign owner, nonparticipant speaker, expired room, or in-flight round.
    pub fn room_transfer(
        &mut self,
        owner: ManagedSessionId,
        id: RoomId,
        speaker: ManagedSessionId,
        now: UnixMillis,
    ) -> Result<(), RoomError> {
        let view = self.room_view(owner, id, now)?;
        if view.owner != owner {
            return Err(RoomError::NotOwner);
        }
        if !view.participants.contains(speaker) {
            return Err(RoomError::NotParticipant(speaker));
        }
        if view.in_flight.is_some() {
            return Err(RoomError::Busy);
        }
        if let Some(room) = self.rooms.get_mut(&id) {
            room.speaker = speaker;
        }
        Ok(())
    }

    /// Admit one fresh ask as the selected speaker. The reply uses the existing exact-message `answer` path.
    ///
    /// Each admitted ask consumes one round, including one later cancelled. Its chain starts fresh, and an
    /// unread reply keeps the round in flight. No next ask is manufactured and no body is retained here.
    ///
    /// # Errors
    /// Refuses invalid room authority, concurrent rounds, the round ceiling, or existing envelope limits.
    pub fn room_ask(
        &mut self,
        speaker: ManagedSessionId,
        id: RoomId,
        target: ManagedSessionId,
        message_id: MessageId,
        body: BoundedUtf8,
        now: UnixMillis,
    ) -> Result<Receipt, RoomError> {
        let view = self.room_view(speaker, id, now)?;
        if view.speaker != speaker {
            return Err(RoomError::NotSpeaker);
        }
        if !view.participants.contains(target) {
            return Err(RoomError::NotParticipant(target));
        }
        if view.rounds >= self.limits.room_rounds {
            return Err(RoomError::RoundBound);
        }
        if view.in_flight.is_some() {
            return Err(RoomError::Busy);
        }
        let mut envelope = CallEnvelope::ask(speaker, target, body, view.deadline);
        envelope.message_id = message_id;
        // The ordinary public send path continues to reject caller-supplied room tags. Only this validated
        // room operation stamps a room after the ordinary shape checks and before routing it atomically.
        self.admit_shape(&envelope, now)?;
        if self.calls.len() >= self.limits.active_calls {
            return Err(Refusal::TooManyCalls {
                ceiling: self.limits.active_calls,
            }
            .into());
        }
        envelope.room_id = Some(id);
        let receipt = self.route(envelope, true)?;
        if let Some(room) = self.rooms.get_mut(&id) {
            room.rounds += 1;
            room.current = Some(CallRef {
                call_id: receipt.call_id,
                ask: receipt.message_id,
            });
        }
        Ok(receipt)
    }

    /// Close a room as its owner, immediately reclaiming only this room's calls and unread bodies.
    ///
    /// # Errors
    /// Refuses an unknown room or an actor other than its owner. An expired room may still be closed.
    pub fn room_close(
        &mut self,
        owner: ManagedSessionId,
        id: RoomId,
    ) -> Result<Released, RoomError> {
        let room = self.rooms.get(&id).ok_or(RoomError::Unknown(id))?;
        if room.owner != owner {
            return Err(RoomError::NotOwner);
        }
        let mut swept = Swept::default();
        self.remove_room(id, DeliveryState::Cancelled, &mut swept);
        Ok(Released {
            envelopes: swept.messages.len(),
            calls: swept.calls.len(),
            bytes: swept.bytes,
        })
    }
}
