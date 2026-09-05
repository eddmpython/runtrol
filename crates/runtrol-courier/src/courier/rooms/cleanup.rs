//! Room retirement releases tagged envelopes and calls, preserving unrelated mailbox order and charges.

use super::{
    CallStage, Courier, DeliveryState, ManagedSessionId, Released, RoomId, Swept, UnixMillis,
};

impl Courier {
    pub(in crate::courier) fn expire_rooms(&mut self, now: UnixMillis, swept: &mut Swept) {
        let expired: Vec<RoomId> = self
            .rooms
            .iter()
            .filter(|(_, room)| room.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.remove_room(id, DeliveryState::Expired, swept);
        }
    }

    pub(in crate::courier) fn end_rooms_of(&mut self, session: ManagedSessionId) -> Released {
        let ended: Vec<RoomId> = self
            .rooms
            .iter()
            .filter(|(_, room)| room.participants.contains(session))
            .map(|(id, _)| *id)
            .collect();
        let mut swept = Swept::default();
        for id in ended {
            self.remove_room(id, DeliveryState::Cancelled, &mut swept);
        }
        Released {
            envelopes: swept.messages.len(),
            calls: swept.calls.len(),
            bytes: swept.bytes,
        }
    }

    pub(super) fn remove_room(&mut self, id: RoomId, outcome: DeliveryState, swept: &mut Swept) {
        if self.rooms.remove(&id).is_none() {
            return;
        }
        let mut dropped = Vec::new();
        for mailbox in self.mailboxes.values_mut() {
            let (removed, kept): (Vec<_>, Vec<_>) = mailbox
                .queue
                .drain(..)
                .partition(|mail| mail.room_id == Some(id));
            let freed: usize = removed.iter().map(|mail| mail.body.len()).sum();
            mailbox.bytes = mailbox.bytes.saturating_sub(freed);
            mailbox.queue.extend(kept);
            dropped.extend(removed);
        }
        for envelope in dropped {
            self.charged_bytes = self.charged_bytes.saturating_sub(envelope.body.len());
            swept.bytes = swept.bytes.saturating_add(envelope.body.len());
            swept.messages.push(envelope.message_id);
            self.retire(envelope.message_id, outcome);
        }
        let ended: Vec<_> = self
            .calls
            .iter()
            .filter(|(_, call)| call.room == Some(id))
            .map(|(id, _)| *id)
            .collect();
        for call_id in ended {
            if let Some(call) = self.calls.remove(&call_id) {
                swept.calls.push(call_id);
                self.retire(
                    call.ask,
                    if call.stage == CallStage::Replied {
                        DeliveryState::Replied
                    } else {
                        outcome
                    },
                );
            }
        }
    }
}
