//! Admitted commands touch one in-memory courier. Waiting releases the state lock and owns no body.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtrol_courier::wire::{Answer, Request, SESSION_PAGE, Session};
use runtrol_courier::{CallEnvelope, CallKind, CallRef, Limits, ManagedSessionId, UnixMillis};

use super::CourierGate;

fn now() -> UnixMillis {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => UnixMillis(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        // Before the epoch no message can be assigned an honest deadline. Zero makes all normal future
        // deadlines exceed the admitted bound and therefore fail closed.
        Err(_before_epoch) => UnixMillis(0),
    }
}

fn refused(reason: impl Into<String>) -> Answer {
    Answer::Refused {
        reason: reason.into(),
    }
}

impl CourierGate {
    pub(super) async fn wait_slot(
        &self,
        session: ManagedSessionId,
    ) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let waits = std::sync::Arc::clone(&self.state.lock().await.sessions.get(&session)?.waits);
        let Ok(slot) = waits.try_acquire_owned() else {
            return None;
        };
        Some(slot)
    }

    #[cfg(test)]
    pub(super) async fn command(&self, session: ManagedSessionId, request: Request) -> Answer {
        self.command_owned(session, request, &mut None).await
    }

    pub(super) async fn command_owned(
        &self,
        session: ManagedSessionId,
        request: Request,
        pending: &mut Option<CallRef>,
    ) -> Answer {
        if let Request::Ask { envelope } = request {
            return self.ask(session, envelope, pending).await;
        }
        if let Request::Receive {
            source,
            call,
            timeout_ms,
        } = request
        {
            *pending = call;
            return self.receive(session, source, call, timeout_ms).await;
        }
        let mut state = self.state.lock().await;
        if !state.courier.is_live(session) {
            return refused("the managed session ended");
        }
        let now = now();
        state.courier.sweep(now);
        let receipt = match request {
            Request::List { after } => {
                let mut rows = state
                    .sessions
                    .iter()
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
            Request::Receive { .. } | Request::Ask { .. } => {
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
        envelope: CallEnvelope,
        pending: &mut Option<CallRef>,
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
            let now = now();
            state.courier.sweep(now);
            let timeout_ms = envelope.deadline.since(now);
            if let Err(error) = state.courier.send(envelope, now) {
                return refused(error.to_string());
            }
            // Only a successfully admitted ask belongs to this connection. Refusing a duplicate must not
            // let its cleanup cancel the original connection's still-live ask.
            *pending = Some(call);
            self.changed.notify_waiters();
            timeout_ms
        };
        self.receive(session, Some(source), Some(call), timeout_ms)
            .await
    }

    async fn receive(
        &self,
        session: ManagedSessionId,
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
                if !state.courier.is_live(session) {
                    return refused("the managed session ended");
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

    pub(super) async fn abandon(&self, session: ManagedSessionId, call: CallRef) {
        let mut state = self.state.lock().await;
        state.courier.sweep(now());
        state.courier.abandon_call(session, call);
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
