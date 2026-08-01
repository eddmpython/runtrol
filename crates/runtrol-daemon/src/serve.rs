//! The daemon, running.
//!
//! # One owner of the sessions, and everything else beside it
//!
//! Every session lives in one [`SessionManager`], and exactly one task ever touches it. That task does two things and
//! races them against each other: it takes the next request that any connection has asked about, and it waits for the
//! next event that any session produces. Everything else about a connection (reading its frames, writing its answers,
//! relaying what it is watching) belongs to that connection's own task, because none of it needs the sessions.
//!
//! The alternative is a lock around the sessions and a task per connection that takes it. That would make the map,
//! the event numbering and the tier bound each a thing two tasks can be inside at once, and every rule the kernel
//! states about ordering would become a rule about lock ordering instead. One owner is what makes those rules true
//! without any locking at all.
//!
//! # Nothing one caller does may stop another
//!
//! The owner task holds the sessions while it answers, so a provider process wait there would stop every session's
//! output. Probes, model discovery, process open, and process cleanup therefore run in connection tasks. The owner
//! only reserves or commits a process slot and synchronously hands an agent to its connection for a command write.
//! [`Reply::Sending`], [`Reply::Stopping`] and [`Reply::Cleaning`] hand every provider wait back out.
//!
//! An operator watching one session while closing another is the case this is for, and it is what the tests here
//! check: a slow close does not stop a running session's events.
//!
//! Connection and cleanup tasks live in one `JoinSet`. Every returned serve outcome aborts and reaps that set. Dropping
//! the serve future drops the set and aborts its tasks; process containment remains the final child teardown boundary.
//!
//! # What is not decided here
//!
//! Who may connect. The endpoint is inside a directory only the operator can enter and remote clients are refused by
//! the transport; the scope wall that reads where a request came from belongs at the dispatch boundary, which is where
//! it goes. This file gets frames to that boundary and answers back.

use core::future::Future;
use core::time::Duration;
use std::sync::Arc;

use runtrol_core::{
    AgentLease, ClosingReservation, OpenReservation, ReservedOpen, SessionError, SessionManager,
    TakenAgent,
};
use runtrol_ipc::transport::{Connection, Listener, TransportError};
use runtrol_ipc::wire::{Request, Response, WireError};
use runtrol_provider::{AgentCommand, CloseMode, Opaque, ProviderError, SessionId};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinSet;

use crate::compose::Composed;
use crate::dispatch::{
    Cleanup, CleanupReservation, Conversation, Discovered, Prepared, PreparedKind, Reply,
    answer_prepared, complete_prepare_for, discover, needs_driver, refuse,
};

/// How many answered requests may be waiting to reach the one task that answers them.
///
/// A bound rather than an unbounded queue, because an unbounded one is a way for a caller to make the daemon grow
/// without limit. A connection that finds it full waits, which is the correct thing for it to do: it has nothing else
/// to be doing until its request is answered.
pub const ASKED_QUEUE: usize = 64;

/// Blocking provider pipe operations the daemon can admit at once on Windows.
///
/// Every hot process may have one stdout read and one stdin write in flight. Discovery and model preparation share
/// one gate and add at most one stdout/stderr pair or model connection pair.
pub const MAX_BLOCKING_PROVIDER_OPERATIONS: usize = runtrol_core::session::MAX_HOT * 2 + 2;

/// Longest model discovery may hold the global provider preparation gate.
///
/// A cold discovery may issue thirteen sequential probes at fifteen seconds each. The remaining allowance covers a
/// normal catalogue response, while deliberately bounding multi-page enumeration rather than adding every internal
/// page timeout together. It also bounds a child that stops reading stdin or a driver that never returns.
pub const MODEL_PREPARATION_BUDGET_MS: u64 = 300_000;

/// The daemon could not keep serving.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ServeError {
    /// The endpoint could not be created or kept.
    #[error(transparent)]
    Transport(#[from] TransportError),

    /// Minimal session metadata could not be persisted.
    #[error(transparent)]
    Store(#[from] runtrol_store::StoreError),
}

/// One request, from a connection that is waiting for the answer.
struct Asked {
    /// The connection's own state, lent for the length of one answer and handed straight back.
    ///
    /// It travels with the request rather than living in the owner task, because it belongs to the connection: an
    /// owner task holding one entry per connection would be a second place a connection's life is recorded, and the
    /// two would disagree the moment one of them missed a disconnect.
    conversation: Conversation,
    /// What was asked.
    request: Request,
    /// Slow provider discovery completed by the connection task, outside the one session owner.
    prepared: Prepared,
    /// The bounded process slot reserved before an open, if this request opens one.
    reservation: Option<ReservationGuard>,
    /// Where the answer goes.
    answered: oneshot::Sender<Answered>,
}

/// A connection asking the session owner for a bounded process slot.
enum ReservationAsked {
    Reserve {
        session: SessionId,
        answered: oneshot::Sender<Result<ReservedOpen, SessionError>>,
    },
    CancelOpen(OpenReservation),
    ReleaseClosing(ClosingReservation),
}

/// Cancels a pending slot if connection preparation is abandoned.
struct ReservationGuard {
    reservation: Option<CleanupReservation>,
    cancelling: mpsc::UnboundedSender<ReservationAsked>,
}

impl ReservationGuard {
    fn take(mut self) -> Option<OpenReservation> {
        match self.reservation.take() {
            Some(CleanupReservation::Open(reservation)) => Some(reservation),
            Some(CleanupReservation::Closing(_)) | None => None,
        }
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            let message = match reservation {
                CleanupReservation::Open(reservation) => ReservationAsked::CancelOpen(reservation),
                CleanupReservation::Closing(reservation) => {
                    ReservationAsked::ReleaseClosing(reservation)
                }
            };
            drop(self.cancelling.send(message));
        }
    }
}

/// One answer, going back to the connection that asked.
struct Answered {
    /// The connection's state, as answering left it.
    conversation: Conversation,
    /// What to do about the request.
    reply: Reply,
}

enum AgentReturned {
    Finished {
        lease: AgentLease,
        agent: Box<dyn runtrol_provider::Agent>,
        outcome: Result<(), ProviderError>,
        answered: oneshot::Sender<Response>,
    },
    Abandoned(AgentLease),
}

struct AgentGuard {
    lease: Option<AgentLease>,
    returning: mpsc::UnboundedSender<AgentReturned>,
}

impl AgentGuard {
    fn take(mut self) -> Option<AgentLease> {
        self.lease.take()
    }
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            drop(self.returning.send(AgentReturned::Abandoned(lease)));
        }
    }
}

/// Serve until the endpoint fails.
///
/// # Errors
///
/// [`ServeError::Transport`] when the endpoint cannot be created or cannot keep accepting. Not worked around: a
/// daemon nothing can reach is a daemon that does nothing, and staying up would hide that from the operator.
pub async fn serve(composed: Composed, mut listener: Listener) -> Result<(), ServeError> {
    let composed = Arc::new(composed);
    let mut sessions = SessionManager::new();
    let (asking, mut asked) = mpsc::channel::<Asked>(ASKED_QUEUE);
    let (reserving, mut reservations) = mpsc::unbounded_channel::<ReservationAsked>();
    let (returning, mut returned) = mpsc::unbounded_channel::<AgentReturned>();
    // ProbeCache replaces one file atomically but is deliberately not a database. Serializing provider preparation
    // keeps two connections from publishing stale snapshots over each other and bounds temporary provider processes.
    // A Models request holds this gate through its provider call. Opens release it after discovery because their
    // process slots are bounded separately by MAX_HOT.
    let discovering = Arc::new(Mutex::new(()));
    let mut connections = JoinSet::new();

    let outcome = loop {
        tokio::select! {
            arrived = listener.accept() => {
                let connection = match arrived {
                    Ok(connection) => connection,
                    Err(error) => break Err(error.into()),
                };
                // The connection's own task. It reads, it writes, and it never touches a session.
                connections.spawn(converse(
                    connection,
                    asking.clone(),
                    reserving.clone(),
                    returning.clone(),
                    Arc::clone(&composed),
                    Arc::clone(&discovering),
                ));
            }

            Some(reservation) = reservations.recv() => match reservation {
                ReservationAsked::Reserve { session, answered } => {
                    if let Err(Ok(abandoned)) = answered.send(sessions.reserve_open(session)) {
                        abandon_reserved(
                            &mut sessions,
                            &mut connections,
                            &reserving,
                            abandoned,
                        );
                    }
                }
                ReservationAsked::CancelOpen(reservation) => sessions.cancel_open(reservation),
                ReservationAsked::ReleaseClosing(reservation) => sessions.release_closing(reservation),
            },

            Some(ask) = asked.recv() => {
                let Asked { mut conversation, request, prepared, reservation, answered } = ask;
                let reservation = reservation.and_then(ReservationGuard::take);
                let reply = answer_prepared(
                    &mut conversation,
                    &composed,
                    &mut sessions,
                    request,
                    prepared,
                    reservation,
                );
                // The connection stopped while its request was being answered. Nothing to report and nowhere to
                // report it: the caller is gone, and the sessions already record everything the request did.
                deliver_answer(
                    answered,
                    Answered { conversation, reply },
                    &mut connections,
                    &reserving,
                    &mut sessions,
                );
            }

            Some(returned_agent) = returned.recv() => match returned_agent {
                AgentReturned::Finished { lease, agent, outcome, answered } => {
                    let response = match sessions.return_agent(lease, agent) {
                        Ok(()) => match outcome {
                            Ok(()) => Response::Done,
                            Err(error) => Response::Failed(runtrol_ipc::wire::WireError::from_provider(&error)),
                        },
                        Err(agent) => {
                            drop(agent);
                            refuse("the session no longer accepts its completed provider command")
                        }
                    };
                    drop(answered.send(response));
                }
                AgentReturned::Abandoned(lease) => sessions.abandon_agent(lease),
            },

            // Events reach whoever is watching through the session's own fan-out, so there is nothing to do with
            // what comes back. What this arm is for is that the reading happens at all.
            pumped = sessions.pump_any() => {
                if let Some(published) = pumped.published {
                    if let Err(error) = crate::dispatch::persist_live(&composed, &sessions, pumped.session) {
                        break Err(error.into());
                    }
                    if let Err(error) = composed.store.put_cursor(
                        pumped.session,
                        runtrol_store::Cursor {
                            src_end: published.event.src_end,
                            seq: published.event.seq,
                        },
                    ) {
                        break Err(error.into());
                    }
                }
            }

            Some(_finished) = connections.join_next(), if !connections.is_empty() => {}
        }
    };

    connections.abort_all();
    while connections.join_next().await.is_some() {}
    outcome
}

/// Release an unanswered reservation without exposing an extra live process during displaced cleanup.
fn abandon_reserved(
    sessions: &mut SessionManager,
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    abandoned: ReservedOpen,
) {
    let ReservedOpen {
        reservation,
        displaced,
    } = abandoned;
    let Some(displaced) = displaced else {
        sessions.cancel_open(reservation);
        return;
    };
    let cancelling = cancelling.clone();
    tasks.spawn(async move {
        let releasing = ReservationGuard {
            reservation: Some(CleanupReservation::Open(reservation)),
            cancelling,
        };
        drop(displaced.close(CloseMode::Graceful { grace_ms: 0 }).await);
        drop(releasing);
    });
}

/// Deliver an answer or finish any process handoff whose connection disappeared first.
fn deliver_answer(
    answered: oneshot::Sender<Answered>,
    answer: Answered,
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    sessions: &mut SessionManager,
) {
    if let Err(abandoned) = answered.send(answer) {
        abandon_reply(tasks, cancelling, sessions, abandoned.reply);
    }
}

fn abandon_reply(
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    sessions: &mut SessionManager,
    reply: Reply,
) {
    match reply {
        Reply::Stopping {
            agent,
            how,
            reservation,
        } => spawn_abandoned_cleanup(
            tasks,
            cancelling,
            agent,
            how,
            Some(CleanupReservation::Closing(reservation)),
        ),
        Reply::Cleaning { agents, .. } => {
            for Cleanup {
                agent,
                how,
                reservation,
            } in agents
            {
                spawn_abandoned_cleanup(tasks, cancelling, agent, how, reservation);
            }
        }
        Reply::Sending { taken, .. } => {
            let TakenAgent { agent, lease } = taken;
            drop(agent);
            sessions.abandon_agent(lease);
        }
        Reply::One(_) | Reply::Watching(_) => {}
    }
}

fn spawn_abandoned_cleanup(
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    agent: Box<dyn runtrol_provider::Agent>,
    how: CloseMode,
    reservation: Option<CleanupReservation>,
) {
    let cancelling = cancelling.clone();
    tasks.spawn(async move {
        let releasing = reservation.map(|reservation| ReservationGuard {
            reservation: Some(reservation),
            cancelling,
        });
        drop(agent.close(how).await);
        drop(releasing);
    });
}

/// One connection, for as long as it lasts.
///
/// Reads a request, asks the one task that owns the sessions, and writes back what it says. A connection that goes
/// away simply ends: it is not a failure the daemon has to act on.
#[expect(
    clippy::too_many_lines,
    reason = "one connection lifecycle keeps reservation cancellation and request ownership visible together"
)]
async fn converse(
    mut connection: Connection,
    asking: mpsc::Sender<Asked>,
    reserving: mpsc::UnboundedSender<ReservationAsked>,
    returning: mpsc::UnboundedSender<AgentReturned>,
    composed: Arc<Composed>,
    discovering: Arc<Mutex<()>>,
) {
    // Every connection today arrives on the local endpoint, which is inside a directory only the operator can
    // enter and refuses anything off this machine. That is what makes this the right answer rather than an
    // assumption: a remote transport arrives with its own way of saying who authenticated, and there is no way to
    // build a conversation that claims to be one until it does.
    let mut conversation = Conversation::at_the_machine();

    loop {
        let frame = match connection.recv().await {
            Ok(Some(frame)) => frame,
            // The other end is gone. Ordinary, and the end of this task.
            Ok(None) => return,
            // The connection failed or sent something this build cannot carry. Said out loud if it can still be
            // written to, and then this connection is over: a stream that produced an unreadable frame cannot be
            // resynchronised, and reading on would act on whatever the next bytes happened to look like.
            Err(error) => {
                drop(write(&mut connection, &refuse(&error.to_string())).await);
                return;
            }
        };

        let request = match serde_json::from_slice::<Request>(&frame) {
            Ok(request) => request,
            // One unreadable request, not a broken connection. Refused by name so the caller can correct it, and
            // the connection stays open because the next request may well be fine.
            Err(error) => {
                if write(&mut connection, &refuse(&error.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };

        let reservation = if matches!(request, Request::Start { .. } | Request::Resume { .. })
            && conversation.greeted()
            && crate::scope::allowed(conversation.caller(), &request, &composed.granted).is_ok()
        {
            let session = SessionId::now();
            let (answered, hearing) = oneshot::channel();
            if reserving
                .send(ReservationAsked::Reserve { session, answered })
                .is_err()
            {
                return;
            }
            let reserved = match hearing.await {
                Ok(Ok(reserved)) => reserved,
                Ok(Err(error)) => {
                    if write(&mut connection, &refuse(&error.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            };
            let ReservedOpen {
                reservation,
                displaced,
            } = reserved;
            let guard = ReservationGuard {
                reservation: Some(CleanupReservation::Open(reservation)),
                cancelling: reserving.clone(),
            };
            if let Some(displaced) = displaced {
                drop(displaced.close(CloseMode::Graceful { grace_ms: 0 }).await);
            }
            Some(guard)
        } else {
            None
        };

        let mut preparation_gate = if needs_driver(&request) {
            Some(discovering.lock().await)
        } else {
            None
        };
        let reserved_session = reservation
            .as_ref()
            .and_then(|guard| guard.reservation.as_ref())
            .map(CleanupReservation::session);
        let prepared = if let Request::Models { provider } = &request {
            let preparing = async {
                let discovered = discover(&conversation, &composed, &request).await;
                complete_prepare_for(&request, discovered, reserved_session).await
            };
            finish_model_preparation(provider, preparing, model_preparation_budget()).await
        } else {
            let discovered = if preparation_gate.is_some() {
                discover(&conversation, &composed, &request).await
            } else {
                Discovered::None
            };
            drop(preparation_gate.take());
            complete_prepare_for(&request, discovered, reserved_session).await
        };
        drop(preparation_gate);
        let (answered, hearing) = oneshot::channel();
        let ask = Asked {
            conversation,
            request,
            prepared,
            reservation,
            answered,
        };
        if asking.send(ask).await.is_err() {
            // The daemon stopped serving. There is nothing left to ask and nothing that could answer.
            return;
        }
        let Ok(back) = hearing.await else {
            // Answering was abandoned, which happens only when the daemon is going away.
            return;
        };
        conversation = back.conversation;

        match back.reply {
            Reply::One(response) => {
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }

            // This connection is a view of a session from here on. It stops when the session's stream ends or when
            // whoever is on the other end goes away, and either way it does not go back to reading requests: a
            // caller that wants both opens two connections, which costs it nothing and keeps this unambiguous.
            Reply::Watching(watching) => {
                // The acknowledgement is the subscription boundary. Without it, a caller can only sleep and guess
                // whether its Watch request arrived before the next prompt, which loses the very event it watches for
                // on a slow machine.
                if write(&mut connection, &Response::Watching).await.is_err() {
                    return;
                }
                relay(&mut connection, *watching).await;
                return;
            }

            // The wait the owner task handed over. Done here so that closing one session does not stop every other
            // session's output, and answered truthfully when it is over rather than optimistically before.
            Reply::Stopping {
                agent,
                how,
                reservation,
            } => {
                let releasing = ReservationGuard {
                    reservation: Some(CleanupReservation::Closing(reservation)),
                    cancelling: reserving.clone(),
                };
                let outcome = match agent.close(how).await {
                    Ok(()) => Response::Done,
                    Err(error) => refuse(&error.to_string()),
                };
                drop(releasing);
                if write(&mut connection, &outcome).await.is_err() {
                    return;
                }
            }

            reply @ Reply::Cleaning { .. } => {
                let response = finish_connection_cleanup(reply, &reserving).await;
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }

            Reply::Sending { taken, command } => {
                let Some(response) = perform_agent_command(taken, command, returning.clone()).await
                else {
                    return;
                };
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Finish a model catalogue without allowing one provider to monopolize preparation forever.
async fn finish_model_preparation<F>(provider: &str, preparing: F, within: Duration) -> Prepared
where
    F: Future<Output = Prepared>,
{
    match tokio::time::timeout(within, preparing).await {
        Ok(prepared) => prepared,
        Err(_elapsed) => Prepared::Invalid {
            kind: PreparedKind::Models,
            provider: provider.into(),
            response: Response::Failed(WireError {
                message: format!(
                    "model discovery for {provider} did not finish within {} milliseconds",
                    within.as_millis()
                )
                .into(),
                retryable: true,
                needs_the_operator: false,
            }),
        },
    }
}

const fn model_preparation_budget() -> Duration {
    Duration::from_millis(MODEL_PREPARATION_BUDGET_MS)
}

/// Perform one provider command outside the session owner, then offer the agent back to it.
#[expect(
    clippy::manual_ok_err,
    reason = "the equivalent Result::ok is forbidden because channel loss must stay visible here"
)]
async fn perform_agent_command(
    taken: TakenAgent,
    command: AgentCommand,
    returning: mpsc::UnboundedSender<AgentReturned>,
) -> Option<Response> {
    let TakenAgent {
        agent: handed_agent,
        lease,
    } = taken;
    let guard = AgentGuard {
        lease: Some(lease),
        returning: returning.clone(),
    };
    // Declared after the guard so cancellation or panic drops the process owner before the guard tells the session
    // owner that its bounded slot may be released. Rust drops locals in reverse declaration order.
    let mut agent = handed_agent;
    let outcome = agent.send(command).await;
    let lease = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(AgentReturned::Finished {
            lease,
            agent,
            outcome,
            answered,
        })
        .is_err()
    {
        return None;
    }
    match hearing.await {
        Ok(response) => Some(response),
        Err(_owner_stopped) => None,
    }
}

async fn finish_connection_cleanup(
    reply: Reply,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
) -> Response {
    let Reply::Cleaning {
        mut response,
        agents,
    } = reply
    else {
        return refuse("connection cleanup received a reply with no process to stop");
    };
    let mut failures = Vec::new();
    for Cleanup {
        agent,
        how,
        reservation,
    } in agents
    {
        let releasing = reservation.map(|reservation| ReservationGuard {
            reservation: Some(reservation),
            cancelling: cancelling.clone(),
        });
        if let Err(error) = agent.close(how).await {
            failures.push(error.to_string());
        }
        drop(releasing);
    }
    if !failures.is_empty()
        && let Response::Failed(error) = &response
    {
        response = refuse(&format!(
            "{}; cleanup also failed: {}",
            error.message,
            failures.join("; ")
        ));
    }
    response
}

/// Relay a session's events to whoever is watching, until one end stops.
///
/// The event goes out as the provider wrote it. Encoded here and read by nobody in between: this is the last hop a
/// conversation takes inside runtrol, and the whole of what happens to it is being put in an envelope.
async fn relay(connection: &mut Connection, mut watching: runtrol_core::SessionView) {
    while let Some(event) = watching.recv().await {
        let encoded = match serde_json::to_string(&event) {
            Ok(encoded) => Opaque::owned(encoded),
            // An event this build cannot write is a defect in this build, and it is about one event rather than
            // about the session. Said out loud in place of that event, because a watcher that silently skipped one
            // would show a conversation with a hole in it and no sign that anything was missing.
            Err(error) => {
                let detail = format!("cannot serialize {} event: {error}", event.body.wire_name());
                drop(write(connection, &refuse(&detail)).await);
                return;
            }
        };
        if write(connection, &Response::Event(encoded)).await.is_err() {
            return;
        }
    }
}

/// Write one answer.
///
/// A response that cannot be serialized is a defect in this build rather than something a caller did, so what goes out
/// instead says exactly that. The alternative is writing nothing, which leaves the caller waiting on a daemon that is
/// working perfectly well.
async fn write(connection: &mut Connection, response: &Response) -> Result<(), TransportError> {
    let frame = serde_json::to_vec(response).unwrap_or_else(|error| {
        let said = refuse(&format!("this daemon could not write its own answer: {error}"));
        serde_json::to_vec(&said).unwrap_or_else(|_| {
            // Two failures to serialize means the failure is in the vocabulary itself. This is that vocabulary,
            // written by hand, so that there is no third thing that could fail.
            br#"{"say":"failed","with":{"message":"this daemon cannot write its own answer","retryable":false,"needs_the_operator":false}}"#.to_vec()
        })
    });
    connection.send(&frame).await
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use runtrol_provider::{Agent, AgentCommand, Produced, ProviderError};

    use super::*;

    struct PendingClose {
        session: SessionId,
        started: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
        panic_after_release: bool,
    }

    struct PendingSend {
        session: SessionId,
        started: Option<oneshot::Sender<()>>,
        release: Option<oneshot::Receiver<()>>,
        panic_after_start: bool,
    }

    #[async_trait]
    impl Agent for PendingSend {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            if let Some(started) = self.started.take() {
                let _started = started.send(());
            }
            assert!(!self.panic_after_start, "scripted provider command panic");
            if let Some(release) = self.release.take() {
                drop(release.await);
            }
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct ReadyEvent {
        session: SessionId,
        ready: bool,
    }

    #[async_trait]
    impl Agent for ReadyEvent {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            if self.ready {
                self.ready = false;
                Some(Ok(Produced {
                    src_end: 1,
                    body: runtrol_provider::EventBody::Plan {
                        payload: Opaque::none(),
                    },
                }))
            } else {
                core::future::pending().await
            }
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn attach_test_agent(sessions: &mut SessionManager, session: SessionId, agent: Box<dyn Agent>) {
        let reserved = sessions.reserve_open(session).expect("one process slot");
        let intent = runtrol_provider::OpenIntent {
            session,
            workspace: runtrol_provider::AbsPath::new(if cfg!(windows) {
                r"C:\work"
            } else {
                "/work"
            })
            .expect("valid test path"),
            disposition: runtrol_provider::Disposition::Fresh,
            model: None,
            permission: None,
        };
        sessions
            .attach_opened(
                reserved.reservation,
                runtrol_provider::ProviderId::parse("test").expect("valid provider"),
                &intent,
                agent,
            )
            .expect("the test process attaches");
    }

    #[async_trait]
    impl Agent for PendingClose {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            let _sent = self.started.send(());
            drop(self.release.await);
            assert!(!self.panic_after_release, "scripted close panic");
            Ok(())
        }
    }

    /// A daemon serving at its own endpoint, and the address to reach it at.
    ///
    /// Every part of this is the real thing: a real endpoint, a real listener, real frames. The one substitution is
    /// the containment, which cannot be established in a test without terminating the runner on one platform.
    struct Running {
        address: String,
        home: String,
        serving: tokio::task::JoinHandle<Result<(), ServeError>>,
    }

    impl Running {
        async fn start(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("runtrol-serve-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            let home = root
                .to_str()
                .expect("the temporary path is UTF-8")
                .to_owned();
            let composed = crate::compose::Composed::for_tests(&home, runtrol_drivers::builtin())
                .expect("a fresh home composes");
            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let serving = tokio::spawn(serve(composed, listener));
            Self {
                address,
                home,
                serving,
            }
        }

        async fn caller(&self) -> Connection {
            runtrol_ipc::transport::connect(&self.address)
                .await
                .expect("the daemon is listening")
        }

        fn stop(self) {
            self.serving.abort();
            drop(std::fs::remove_dir_all(&self.home));
        }
    }

    /// Ask, and read the answer.
    async fn ask(connection: &mut Connection, request: &Request) -> Response {
        let frame = serde_json::to_vec(request).expect("writable");
        connection.send(&frame).await.expect("the daemon is there");
        let answer = connection
            .recv()
            .await
            .expect("the connection holds")
            .expect("every request produces an answer");
        serde_json::from_slice(&answer).expect("the answer is readable")
    }

    #[tokio::test]
    async fn a_stuck_model_provider_releases_global_preparation() {
        struct DropSignal(Option<oneshot::Sender<()>>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(dropped) = self.0.take() {
                    let _dropped = dropped.send(());
                }
            }
        }

        let gate = Arc::new(Mutex::new(()));
        let first_gate = Arc::clone(&gate);
        let (held, holding) = oneshot::channel();
        let (dropped, dropping) = oneshot::channel();
        let first = tokio::spawn(async move {
            let _guard = first_gate.lock().await;
            held.send(()).expect("the test observes the held gate");
            let preparing = async move {
                let _cleanup = DropSignal(Some(dropped));
                core::future::pending::<Prepared>().await
            };
            finish_model_preparation("fixture", preparing, Duration::from_millis(25)).await
        });
        holding.await.expect("the first preparation holds the gate");

        let second_gate = Arc::clone(&gate);
        let second = tokio::spawn(async move {
            let _guard = second_gate.lock().await;
        });

        let prepared = first.await.expect("the bounded preparation task finishes");
        match prepared {
            Prepared::Invalid {
                kind: PreparedKind::Models,
                provider,
                response: Response::Failed(error),
            } => {
                assert_eq!(&*provider, "fixture");
                assert!(error.retryable);
                assert!(!error.needs_the_operator);
                assert!(error.message.contains("did not finish"));
            }
            _ => panic!("the stuck model preparation must become a bound refusal"),
        }
        dropping
            .await
            .expect("timeout drops provider preparation before releasing its gate");
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("the next preparation acquired the released gate")
            .expect("the next preparation task finishes");
    }

    #[tokio::test]
    async fn a_request_over_a_real_endpoint_is_answered() {
        // Not a unit of the loop: an actual listener, an actual connection, actual frames. Everything below is the
        // same daemon an operator reaches.
        let running = Running::start("answered").await;
        let mut caller = running.caller().await;

        let welcome = ask(
            &mut caller,
            &Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(matches!(welcome, Response::Welcome { .. }), "{welcome:?}");

        match ask(&mut caller, &Request::List).await {
            Response::Sessions(listing) => {
                assert!(listing.sessions.is_empty(), "nothing has been started");
                assert!(listing.warnings.is_empty(), "a fresh store has no warnings");
            }
            other => panic!("expected a listing, got {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn the_greeting_is_enforced_on_the_wire_and_not_only_in_the_dispatcher() {
        // The rule exists in the dispatcher, and this checks it survives the trip: a connection is refused because
        // of what it has not done, not because of anything this file remembered about it.
        let running = Running::start("ungreeted").await;
        let mut caller = running.caller().await;

        match ask(&mut caller, &Request::List).await {
            Response::Failed(failure) => assert!(
                failure.message.contains("wire format"),
                "{}",
                failure.message
            ),
            other => panic!("expected a refusal, got {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn every_connection_greets_for_itself() {
        // A second place a connection's state could live is a second place it could be wrong. One connection having
        // greeted must say nothing about another, or a caller inherits permission it never asked for.
        let running = Running::start("separate").await;
        let mut greeted = running.caller().await;
        drop(
            ask(
                &mut greeted,
                &Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await,
        );

        let mut fresh = running.caller().await;
        match ask(&mut fresh, &Request::List).await {
            Response::Failed(failure) => assert!(
                failure.message.contains("wire format"),
                "{}",
                failure.message
            ),
            other => panic!("a fresh connection inherited a greeting: {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn a_frame_that_is_not_a_request_is_refused_and_the_connection_survives() {
        // One bad request is not a broken connection. Closing on it would make a caller's typo look like the daemon
        // dying, and the next request is usually fine.
        let running = Running::start("garbage").await;
        let mut caller = running.caller().await;

        caller
            .send(b"this is not a request")
            .await
            .expect("the daemon is there");
        let answer = caller
            .recv()
            .await
            .expect("the connection holds")
            .expect("even nonsense is answered");
        let read: Response = serde_json::from_slice(&answer).expect("the answer is readable");
        assert!(matches!(read, Response::Failed(_)), "{read:?}");

        let welcome = ask(
            &mut caller,
            &Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(
            matches!(welcome, Response::Welcome { .. }),
            "the connection had to survive one unreadable frame: {welcome:?}"
        );
        running.stop();
    }

    #[tokio::test]
    async fn one_caller_waiting_does_not_stop_another_from_being_answered() {
        // Several connections at once is the ordinary case (a terminal watching, a phone listing), and the whole
        // arrangement of this file is for it. A daemon that answered one connection at a time would be a daemon
        // that freezes whenever anything is slow.
        let running = Running::start("concurrent").await;
        let mut first = running.caller().await;
        let mut second = running.caller().await;

        for caller in [&mut first, &mut second] {
            let welcome = ask(
                caller,
                &Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await;
            assert!(matches!(welcome, Response::Welcome { .. }));
        }

        // Interleaved on purpose: each answer has to come back on the connection that asked for it.
        assert!(matches!(
            ask(&mut second, &Request::List).await,
            Response::Sessions(_)
        ));
        assert!(matches!(
            ask(&mut first, &Request::List).await,
            Response::Sessions(_)
        ));
        running.stop();
    }

    #[tokio::test]
    async fn a_pending_provider_write_does_not_block_another_event_or_owner_request() {
        let mut sessions = SessionManager::new();
        let command_session = SessionId::now();
        let event_session = SessionId::now();
        let (started, starting) = oneshot::channel();
        let (release, releasing) = oneshot::channel();
        attach_test_agent(
            &mut sessions,
            command_session,
            Box::new(PendingSend {
                session: command_session,
                started: Some(started),
                release: Some(releasing),
                panic_after_start: false,
            }),
        );
        attach_test_agent(
            &mut sessions,
            event_session,
            Box::new(ReadyEvent {
                session: event_session,
                ready: true,
            }),
        );

        let taken = sessions
            .take_agent(command_session)
            .expect("the command is handed to its connection");
        let (returning, mut returned) = mpsc::unbounded_channel();
        let command = tokio::spawn(perform_agent_command(
            taken,
            AgentCommand::Interrupt,
            returning,
        ));
        tokio::time::timeout(core::time::Duration::from_secs(2), starting)
            .await
            .expect("provider command start did not time out")
            .expect("provider command started");

        assert!(matches!(
            sessions.take_agent(command_session),
            Err(SessionError::AgentInFlight { session }) if session == command_session
        ));
        let pumped = tokio::time::timeout(
            core::time::Duration::from_secs(2),
            sessions.pump_once(event_session),
        )
        .await
        .expect("another event pump did not time out")
        .expect("the other session remains live");
        assert!(pumped.is_some(), "the other session's event was published");

        let mut reservations = Vec::new();
        for _ in 0..runtrol_core::session::MAX_HOT - 2 {
            reservations.push(
                sessions
                    .reserve_open(SessionId::now())
                    .expect("an unrelated owner request progresses"),
            );
        }
        assert!(matches!(
            sessions.reserve_open(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("the provider command may finish");
        let returned_agent =
            tokio::time::timeout(core::time::Duration::from_secs(2), returned.recv())
                .await
                .expect("provider return did not time out")
                .expect("the provider returned its agent");
        let AgentReturned::Finished {
            lease,
            agent,
            outcome,
            answered,
        } = returned_agent
        else {
            panic!("a completed provider command was expected");
        };
        assert!(outcome.is_ok());
        assert!(
            sessions.return_agent(lease, agent).is_ok(),
            "the owner restores the exact agent"
        );
        answered
            .send(Response::Done)
            .expect("the worker is waiting");
        assert!(matches!(
            tokio::time::timeout(core::time::Duration::from_secs(2), command)
                .await
                .expect("command completion did not time out")
                .expect("command task completed"),
            Some(Response::Done)
        ));

        for reserved in reservations {
            sessions.cancel_open(reserved.reservation);
        }
    }

    #[tokio::test]
    async fn a_cancelled_or_panicking_provider_command_never_reattaches_its_agent() {
        for panic_after_start in [false, true] {
            let mut sessions = SessionManager::new();
            let session = SessionId::now();
            let (started, starting) = oneshot::channel();
            let (_release, releasing) = oneshot::channel();
            attach_test_agent(
                &mut sessions,
                session,
                Box::new(PendingSend {
                    session,
                    started: Some(started),
                    release: Some(releasing),
                    panic_after_start,
                }),
            );
            let taken = sessions
                .take_agent(session)
                .expect("the agent is handed out");
            let (returning, mut returned) = mpsc::unbounded_channel();
            let command = tokio::spawn(perform_agent_command(
                taken,
                AgentCommand::Interrupt,
                returning,
            ));
            tokio::time::timeout(core::time::Duration::from_secs(2), starting)
                .await
                .expect("provider command start did not time out")
                .expect("provider command started");
            let joined = if panic_after_start {
                tokio::time::timeout(core::time::Duration::from_secs(2), command)
                    .await
                    .expect("panicking command join did not time out")
            } else {
                command.abort();
                tokio::time::timeout(core::time::Duration::from_secs(2), command)
                    .await
                    .expect("cancelled command join did not time out")
            };
            if panic_after_start {
                assert!(joined.expect_err("the command panics").is_panic());
            } else {
                assert!(joined.expect_err("the command is cancelled").is_cancelled());
            }

            let abandoned =
                tokio::time::timeout(core::time::Duration::from_secs(2), returned.recv())
                    .await
                    .expect("abandoned handoff did not time out")
                    .expect("the guard reports its abandoned lease");
            let AgentReturned::Abandoned(lease) = abandoned else {
                panic!("an abandoned provider command was expected");
            };
            sessions.abandon_agent(lease);
            assert!(!sessions.is_live(session));
            assert!(
                sessions.reserve_open(SessionId::now()).is_ok(),
                "cleanup returns the process slot"
            );
        }
    }

    #[test]
    fn abandoning_connection_preparation_requests_reservation_cancellation() {
        let mut sessions = SessionManager::new();
        let reserved = sessions
            .reserve_open(SessionId::now())
            .expect("one bounded slot");
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        drop(ReservationGuard {
            reservation: Some(CleanupReservation::Open(reserved.reservation)),
            cancelling,
        });

        let ReservationAsked::CancelOpen(reservation) = cancelled
            .try_recv()
            .expect("dropping preparation reports its reservation")
        else {
            panic!("a cancellation message was expected");
        };
        sessions.cancel_open(reservation);
        for _ in 0..runtrol_core::session::MAX_HOT {
            sessions
                .reserve_open(SessionId::now())
                .expect("the abandoned slot was returned");
        }
    }

    #[tokio::test]
    async fn an_unanswered_reservation_stays_occupied_while_displaced_cleanup_is_pending() {
        let mut sessions = SessionManager::new();
        let mut held = Vec::new();
        for _ in 0..runtrol_core::session::MAX_HOT {
            held.push(
                sessions
                    .reserve_open(SessionId::now())
                    .expect("fills one bounded slot"),
            );
        }
        let abandoned = held.pop().expect("one reserved slot");
        let (started, closing) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let abandoned = ReservedOpen {
            reservation: abandoned.reservation,
            displaced: Some(Box::new(PendingClose {
                session: SessionId::now(),
                started,
                release: released,
                panic_after_release: false,
            })),
        };
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        let mut tasks = JoinSet::new();

        abandon_reserved(&mut sessions, &mut tasks, &cancelling, abandoned);
        tokio::time::timeout(core::time::Duration::from_secs(2), closing)
            .await
            .expect("cleanup start did not time out")
            .expect("cleanup started");
        assert!(matches!(
            sessions.reserve_open(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("cleanup may finish");
        tokio::time::timeout(core::time::Duration::from_secs(2), tasks.join_next())
            .await
            .expect("cleanup task join did not time out")
            .expect("cleanup task joined")
            .expect("cleanup task completed");
        let ReservationAsked::CancelOpen(reservation) =
            tokio::time::timeout(core::time::Duration::from_secs(2), cancelled.recv())
                .await
                .expect("slot release did not time out")
                .expect("cleanup releases its slot")
        else {
            panic!("a cancellation message was expected");
        };
        sessions.cancel_open(reservation);
        assert!(sessions.reserve_open(SessionId::now()).is_ok());
    }

    enum DroppedCleanup {
        Stopping,
        Cleaning,
    }

    async fn dropped_answer_holds_slot_until_cleanup(
        kind: DroppedCleanup,
        panic_after_release: bool,
    ) {
        let mut sessions = SessionManager::new();
        let mut held = Vec::new();
        for _ in 0..runtrol_core::session::MAX_HOT {
            held.push(
                sessions
                    .reserve_open(SessionId::now())
                    .expect("fills one bounded slot"),
            );
        }
        let reserved = held.pop().expect("one reserved slot").reservation;
        let (started, closing) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let agent: Box<dyn Agent> = Box::new(PendingClose {
            session: reserved.session(),
            started,
            release: released,
            panic_after_release,
        });
        let reply = match kind {
            DroppedCleanup::Stopping => {
                let intent = runtrol_provider::OpenIntent {
                    session: reserved.session(),
                    workspace: runtrol_provider::AbsPath::new(if cfg!(windows) {
                        r"C:\work"
                    } else {
                        "/work"
                    })
                    .expect("valid test path"),
                    disposition: runtrol_provider::Disposition::Fresh,
                    model: None,
                    permission: None,
                };
                sessions
                    .attach_opened(
                        reserved,
                        runtrol_provider::ProviderId::parse("test").expect("valid provider"),
                        &intent,
                        agent,
                    )
                    .expect("the cleanup fixture attaches");
                let closing = sessions
                    .close(intent.session)
                    .expect("the cleanup fixture starts closing");
                Reply::Stopping {
                    agent: closing.agent,
                    how: CloseMode::Kill,
                    reservation: closing.reservation,
                }
            }
            DroppedCleanup::Cleaning => Reply::Cleaning {
                response: Response::Done,
                agents: vec![Cleanup {
                    agent,
                    how: CloseMode::Kill,
                    reservation: Some(CleanupReservation::Open(reserved)),
                }],
            },
        };
        let answer = Answered {
            conversation: Conversation::at_the_machine(),
            reply,
        };
        let (answered, hearing) = oneshot::channel();
        drop(hearing);
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        let mut tasks = JoinSet::new();

        deliver_answer(answered, answer, &mut tasks, &cancelling, &mut sessions);
        tokio::time::timeout(core::time::Duration::from_secs(2), closing)
            .await
            .expect("abandoned cleanup start did not time out")
            .expect("abandoned reply cleanup started");
        assert!(matches!(
            sessions.reserve_open(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("cleanup may finish");
        let joined = tokio::time::timeout(core::time::Duration::from_secs(2), tasks.join_next())
            .await
            .expect("cleanup task join did not time out")
            .expect("cleanup task joined");
        if panic_after_release {
            assert!(joined.is_err(), "the scripted cleanup had to panic");
        } else {
            joined.expect("cleanup task completed");
        }
        let released = tokio::time::timeout(core::time::Duration::from_secs(2), cancelled.recv())
            .await
            .expect("cleanup slot release did not time out")
            .expect("cleanup releases its slot");
        match released {
            ReservationAsked::CancelOpen(reservation) => sessions.cancel_open(reservation),
            ReservationAsked::ReleaseClosing(reservation) => {
                sessions.release_closing(reservation);
            }
            ReservationAsked::Reserve { .. } => panic!("a slot release was expected"),
        }
        assert!(sessions.reserve_open(SessionId::now()).is_ok());
    }

    #[tokio::test]
    async fn a_dropped_stopping_answer_keeps_its_slot_until_cleanup() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Stopping, false).await;
    }

    #[tokio::test]
    async fn a_dropped_cleaning_answer_keeps_its_slot_until_cleanup() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Cleaning, false).await;
    }

    #[tokio::test]
    async fn a_panicking_abandoned_cleanup_still_releases_its_slot() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Stopping, true).await;
    }

    #[test]
    fn a_refusal_this_file_writes_is_readable_by_the_surface_that_reads_answers() {
        // Written by this file rather than by the vocabulary, so its shape is worth checking rather than assuming.
        let said = refuse("something went wrong");
        let bytes = serde_json::to_vec(&said).expect("writable");
        let read: Response = serde_json::from_slice(&bytes).expect("readable");
        match read {
            Response::Failed(error) => assert_eq!(&*error.message, "something went wrong"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_last_resort_answer_is_a_refusal_and_not_something_unreadable() {
        // The one answer written by hand. If it were wrong, the case it exists for would be a caller waiting
        // forever, which is the outcome this whole file is arranged to prevent.
        let bytes = br#"{"say":"failed","with":{"message":"this daemon cannot write its own answer","retryable":false,"needs_the_operator":false}}"#;
        let read: Response =
            serde_json::from_slice(bytes).expect("the last resort must be readable");
        assert!(matches!(read, Response::Failed(_)));
    }
}
