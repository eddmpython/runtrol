//! Room commands delegate every participant and speaker decision to the bounded room core.

use runtrol_courier::wire::{Answer, Request};
use runtrol_courier::{BoundedUtf8, CallRef, Limits, ManagedSessionId, MessageId, RoomId};

use super::commands::{active, now, refused};
use super::{Admitted, CourierGate, PendingCall};

impl CourierGate {
    pub(super) async fn room_command(&self, admitted: Admitted, request: Request) -> Answer {
        let session = admitted.session;
        let Some(activation) = admitted.activation else {
            return refused("dialogue was disabled when this connection was admitted");
        };
        let mut state = self.state.lock().await;
        if !active(&state, session, activation) {
            return refused("this dialogue activation ended");
        }
        let now = now();
        let swept = state.courier.sweep(now);
        if !swept.messages.is_empty() || !swept.calls.is_empty() {
            self.changed.notify_waiters();
        }
        match request {
            Request::RoomOpen {
                participants,
                deadline,
            } => {
                let result = state
                    .courier
                    .room_open(session, &participants, deadline, now)
                    .and_then(|room| state.courier.room_view(session, room, now));
                if result.is_ok() {
                    self.changed.notify_waiters();
                }
                match result {
                    Ok(room) => Answer::Room { room },
                    Err(error) => refused(error.to_string()),
                }
            }
            Request::RoomInspect { room } => match state.courier.room_view(session, room, now) {
                Ok(room) => Answer::Room { room },
                Err(error) => refused(error.to_string()),
            },
            Request::RoomTransfer { room, speaker } => {
                let result = state
                    .courier
                    .room_transfer(session, room, speaker, now)
                    .and_then(|()| state.courier.room_view(session, room, now));
                if result.is_ok() {
                    self.changed.notify_waiters();
                }
                match result {
                    Ok(room) => Answer::Room { room },
                    Err(error) => refused(error.to_string()),
                }
            }
            Request::RoomClose { room } => {
                let result = state.courier.room_close(session, room);
                if result.is_ok() {
                    self.changed.notify_waiters();
                }
                match result {
                    Ok(_) => Answer::RoomClosed { room },
                    Err(error) => refused(error.to_string()),
                }
            }
            _ => refused("non-room command reached room dispatch"),
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one admitted round keeps its exact body, target, wait and cleanup adjacent"
    )]
    pub(super) async fn room_ask(
        &self,
        admitted: Admitted,
        room: RoomId,
        target: ManagedSessionId,
        message_id: MessageId,
        body: BoundedUtf8,
        timeout_ms: u64,
        pending: &mut Option<PendingCall>,
    ) -> Answer {
        if timeout_ms == 0 || timeout_ms > Limits::INITIAL.max_deadline_millis {
            return refused("room ask requires a positive bounded wait");
        }
        let Some(activation) = admitted.activation else {
            return refused("dialogue was disabled when this connection was admitted");
        };
        let session = admitted.session;
        let (call, timeout_ms) = {
            let mut state = self.state.lock().await;
            if !active(&state, session, activation) {
                return refused("this dialogue activation ended");
            }
            let now = now();
            state.courier.sweep(now);
            let deadline = match state.courier.room_view(session, room, now) {
                Ok(view) => view.deadline,
                Err(error) => return refused(error.to_string()),
            };
            let receipt = match state
                .courier
                .room_ask(session, room, target, message_id, body, now)
            {
                Ok(receipt) => receipt,
                Err(error) => return refused(error.to_string()),
            };
            let call = CallRef {
                call_id: receipt.call_id,
                ask: receipt.message_id,
            };
            // A refusal never gets cleanup authority over an already-running round.
            *pending = Some(PendingCall { activation, call });
            let timeout_ms = timeout_ms.min(deadline.since(now));
            self.changed.notify_waiters();
            (call, timeout_ms)
        };
        self.receive(session, activation, Some(target), Some(call), timeout_ms)
            .await
    }
}
