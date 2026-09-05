//! Admitted commands touch one in-memory courier. Waiting releases the state lock and owns no body.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtrol_courier::wire::{Answer, Request, SESSION_PAGE, Session};
use runtrol_courier::{CallEnvelope, CallKind, CallRef, Limits, ManagedSessionId, UnixMillis};

use super::{Admitted, CourierGate, GateState, PendingCall};

pub(super) fn active(state: &GateState, session: ManagedSessionId, activation: u64) -> bool {
    state
        .sessions
        .get(&session)
        .is_some_and(|registered| registered.enabled && registered.activation == activation)
}

pub(super) fn now() -> UnixMillis {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => UnixMillis(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        // Before the epoch no message can be assigned an honest deadline. Zero makes all normal future
        // deadlines exceed the admitted bound and therefore fail closed.
        Err(_before_epoch) => UnixMillis(0),
    }
}

pub(super) fn refused(reason: impl Into<String>) -> Answer {
    Answer::Refused {
        reason: reason.into(),
    }
}

impl CourierGate {
    pub(super) async fn wait_slot(
        &self,
        admitted: Admitted,
    ) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let activation = admitted.activation?;
        let waits = {
            let state = self.state.lock().await;
            let registered = state.sessions.get(&admitted.session)?;
            if !registered.enabled || registered.activation != activation {
                return None;
            }
            std::sync::Arc::clone(&registered.waits)
        };
        let Ok(slot) = waits.try_acquire_owned() else {
            return None;
        };
        Some(slot)
    }

    #[cfg(test)]
    pub(super) async fn command(&self, session: ManagedSessionId, request: Request) -> Answer {
        let admitted = {
            let state = self.state.lock().await;
            let Some(registered) = state.sessions.get(&session) else {
                return refused("the managed session ended");
            };
            registered.admission(session)
        };
        self.command_owned(admitted, request, &mut None).await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the closed wire table routes every command through its admitted activation"
    )]
    pub(super) async fn command_owned(
        &self,
        admitted: Admitted,
        request: Request,
        pending: &mut Option<PendingCall>,
    ) -> Answer {
        let session = admitted.session;
        let Some(activation) = admitted.activation else {
            return refused("dialogue was disabled when this connection was admitted");
        };
        if let Request::RoomAsk {
            room,
            target,
            message_id,
            body,
            timeout_ms,
        } = request
        {
            return self
                .room_ask(
                    admitted, room, target, message_id, body, timeout_ms, pending,
                )
                .await;
        }
        if matches!(
            &request,
            Request::RoomOpen { .. }
                | Request::RoomInspect { .. }
                | Request::RoomTransfer { .. }
                | Request::RoomClose { .. }
        ) {
            return self.room_command(admitted, request).await;
        }
        if let Request::Ask { envelope } = request {
            return self.ask(session, activation, envelope, pending).await;
        }
        if let Request::Receive {
            source,
            call,
            timeout_ms,
        } = request
        {
            if timeout_ms > Limits::INITIAL.max_deadline_millis {
                return refused("wait exceeds the courier deadline ceiling");
            }
            *pending = call.map(|call| PendingCall { activation, call });
            return self
                .receive(session, activation, source, call, timeout_ms)
                .await;
        }
        let mut state = self.state.lock().await;
        if !active(&state, session, activation) {
            return refused("this dialogue activation ended");
        }
        let now = now();
        state.courier.sweep(now);
        let receipt = match request {
            Request::List { after } => {
                let mut rows = state
                    .sessions
                    .iter()
                    .filter(|(_, registered)| registered.enabled)
                    .filter(|(id, _)| after.is_none_or(|after| **id > after))
                    .filter_map(|(session, registered)| {
                        registered.root.map(|root| Session {
                            session: *session,
                            pid: root.pid(),
                        })
                    });
                let sessions: Vec<Session> = rows.by_ref().take(SESSION_PAGE).collect();
                let next = rows
                    .next()
                    .and_then(|_| sessions.last().map(|row| row.session));
                return Answer::Sessions { sessions, next };
            }
            Request::Send { envelope } => {
                if envelope.source != session {
                    return refused("the envelope source is not the admitted session");
                }
                state.courier.send(envelope, now)
            }
            Request::Reply {
                message,
                message_id,
                body,
            } => state
                .courier
                .answer(session, message, message_id, body, now),
            Request::Cancel { call, message_id } => {
                state.courier.cancel_call(session, call, message_id, now)
            }
            Request::Spawn { .. }
            | Request::Receive { .. }
            | Request::Ask { .. }
            | Request::RoomAsk { .. }
            | Request::RoomOpen { .. }
            | Request::RoomInspect { .. }
            | Request::RoomTransfer { .. }
            | Request::RoomClose { .. } => {
                return refused("receive requires the asynchronous wait path");
            }
        };
        self.changed.notify_waiters();
        match receipt {
            Ok(receipt) => Answer::Accepted { receipt },
            Err(error) => refused(error.to_string()),
        }
    }

    async fn ask(
        &self,
        session: ManagedSessionId,
        activation: u64,
        envelope: CallEnvelope,
        pending: &mut Option<PendingCall>,
    ) -> Answer {
        if envelope.source != session || envelope.kind != CallKind::Ask {
            return refused("an ask must name its admitted source and ask kind");
        }
        let call = CallRef {
            call_id: envelope.call_id,
            ask: envelope.message_id,
        };
        let source = envelope.target;
        let timeout_ms = {
            let mut state = self.state.lock().await;
            if !active(&state, session, activation) {
                return refused("this dialogue activation ended");
            }
            let now = now();
            state.courier.sweep(now);
            let timeout_ms = envelope.deadline.since(now);
            if let Err(error) = state.courier.send(envelope, now) {
                return refused(error.to_string());
            }
            // Only a successfully admitted ask belongs to this connection. Refusing a duplicate must not
            // let its cleanup cancel the original connection's still-live ask.
            *pending = Some(PendingCall { activation, call });
            self.changed.notify_waiters();
            timeout_ms
        };
        self.receive(session, activation, Some(source), Some(call), timeout_ms)
            .await
    }

    pub(super) async fn receive(
        &self,
        session: ManagedSessionId,
        activation: u64,
        source: Option<ManagedSessionId>,
        call: Option<CallRef>,
        timeout_ms: u64,
    ) -> Answer {
        if timeout_ms > Limits::INITIAL.max_deadline_millis {
            return refused("wait exceeds the courier deadline ceiling");
        }
        let until = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // Register before checking the mailbox so a send between the check and sleep cannot be lost.
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().await;
                if !active(&state, session, activation) {
                    return refused("this dialogue activation ended");
                }
                let now = now();
                let swept = state.courier.sweep(now);
                if !swept.messages.is_empty() || !swept.calls.is_empty() {
                    self.changed.notify_waiters();
                }
                if timeout_ms > 0 && tokio::time::Instant::now() >= until {
                    return Answer::Received { envelope: None };
                }
                if let Some(envelope) = state.courier.receive_matching(session, source, call, now) {
                    self.changed.notify_waiters();
                    return Answer::Received {
                        envelope: Some(envelope),
                    };
                }
                if call.is_some_and(|call| !state.courier.owns_call(session, call)) {
                    return refused("the exact call ended or is not owned by this session");
                }
            }
            if tokio::time::Instant::now() >= until {
                return Answer::Received { envelope: None };
            }
            tokio::select! {
                () = &mut changed => {},
                () = tokio::time::sleep_until(until) => {},
            }
        }
    }

    pub(super) async fn abandon(&self, session: ManagedSessionId, pending: PendingCall) {
        let mut state = self.state.lock().await;
        if !active(&state, session, pending.activation) {
            return;
        }
        state.courier.sweep(now());
        state.courier.abandon_call(session, pending.call);
        self.changed.notify_waiters();
    }

    /// Sleep until the nearest actual expiry. An idle courier has no polling timer or task activity.
    pub(super) async fn expire(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let deadline = self.state.lock().await.courier.next_deadline();
            if let Some(deadline) = deadline {
                tokio::select! {
                    () = &mut changed => continue,
                    () = tokio::time::sleep(Duration::from_millis(deadline.since(now()))) => {},
                }
                self.state.lock().await.courier.sweep(now());
                self.changed.notify_waiters();
            } else {
                changed.await;
            }
        }
    }
}
