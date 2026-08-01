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
//! The owner task holds the sessions while it answers, so any request that waits is a request that stops every
//! session's output for as long as it waits. There is exactly one such request, and it is the reason
//! [`Reply::Stopping`] exists: closing a session gives its process time to finish, and that wait is handed to the
//! connection that asked for it. What the owner does is bounded by starting a process and writing a line.
//!
//! An operator watching one session while closing another is the case this is for, and it is what the tests here
//! check: a slow close does not stop a running session's events.
//!
//! # What is not decided here
//!
//! Who may connect. The endpoint is inside a directory only the operator can enter and remote clients are refused by
//! the transport; the scope wall that reads where a request came from belongs at the dispatch boundary, which is where
//! it goes. This file gets frames to that boundary and answers back.

use std::sync::Arc;

use runtrol_core::SessionManager;
use runtrol_ipc::transport::{Connection, Listener, TransportError};
use runtrol_ipc::wire::{Request, Response};
use runtrol_provider::Opaque;
use tokio::sync::{mpsc, oneshot};

use crate::compose::Composed;
use crate::dispatch::{Conversation, Reply, answer, refuse};

/// How many answered requests may be waiting to reach the one task that answers them.
///
/// A bound rather than an unbounded queue, because an unbounded one is a way for a caller to make the daemon grow
/// without limit. A connection that finds it full waits, which is the correct thing for it to do: it has nothing else
/// to be doing until its request is answered.
pub const ASKED_QUEUE: usize = 64;

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
    /// Where the answer goes.
    answered: oneshot::Sender<Answered>,
}

/// One answer, going back to the connection that asked.
struct Answered {
    /// The connection's state, as answering left it.
    conversation: Conversation,
    /// What to do about the request.
    reply: Reply,
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

    loop {
        tokio::select! {
            arrived = listener.accept() => {
                let connection = arrived?;
                // The connection's own task. It reads, it writes, and it never touches a session.
                drop(tokio::spawn(converse(connection, asking.clone())));
            }

            Some(ask) = asked.recv() => {
                let Asked { mut conversation, request, answered } = ask;
                let reply = answer(&mut conversation, &composed, &mut sessions, request).await;
                // The connection stopped while its request was being answered. Nothing to report and nowhere to
                // report it: the caller is gone, and the sessions already record everything the request did.
                drop(answered.send(Answered { conversation, reply }));
            }

            // Events reach whoever is watching through the session's own fan-out, so there is nothing to do with
            // what comes back. What this arm is for is that the reading happens at all.
            pumped = sessions.pump_any() => {
                if let Some(published) = pumped.published {
                    crate::dispatch::persist_live(&composed, &sessions, pumped.session)?;
                    composed.store.put_cursor(
                        pumped.session,
                        runtrol_store::Cursor {
                            src_end: published.event.src_end,
                            seq: published.event.seq,
                        },
                    )?;
                }
            }
        }
    }
}

/// One connection, for as long as it lasts.
///
/// Reads a request, asks the one task that owns the sessions, and writes back what it says. A connection that goes
/// away simply ends: it is not a failure the daemon has to act on.
async fn converse(mut connection: Connection, asking: mpsc::Sender<Asked>) {
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

        let (answered, hearing) = oneshot::channel();
        let ask = Asked {
            conversation,
            request,
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
                relay(&mut connection, *watching).await;
                return;
            }

            // The wait the owner task handed over. Done here so that closing one session does not stop every other
            // session's output, and answered truthfully when it is over rather than optimistically before.
            Reply::Stopping { agent, how } => {
                let outcome = match agent.close(how).await {
                    Ok(()) => Response::Done,
                    Err(error) => refuse(&error.to_string()),
                };
                if write(&mut connection, &outcome).await.is_err() {
                    return;
                }
            }
        }
    }
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
                drop(write(connection, &refuse(&error.to_string())).await);
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
    use super::*;

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
