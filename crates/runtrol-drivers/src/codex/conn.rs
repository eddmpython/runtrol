//! One process, every session on it, and the demultiplexer in between.
//!
//! # Why this file exists at all
//!
//! The other supported CLI runs one process per session and needs nothing like this. This one is a daemon:
//! measured, a single `app-server` multiplexes every conversation over one pair of standard streams. That is
//! what makes N sessions cost one child, which is the memory contract's whole reason for preferring it, and it
//! is what forces the three problems this file owns.
//!
//! **Frames have to be sorted.** Almost every notification names the conversation it belongs to, so a reader
//! task classifies each line and hands it to that conversation's queue. The one bound notification that names
//! none is account state, which is true of every session at once and goes to all of them.
//!
//! **A slow session must not stall the others.** One queue per session, each bounded. A session whose queue is
//! full has frames dropped rather than made to wait, because making the reader wait would stop every other
//! session on the connection. What was dropped is counted and the session is told, which is the difference
//! between backpressure and losing somebody's output quietly.
//!
//! **Every question gets an answer.** Eleven methods run the other way, and one left open stalls the daemon
//! and therefore all of its sessions. So an incoming request is always answered, whether or not runtrol has a
//! binding for it.
//!
//! # What is deliberately not here
//!
//! A line that is not a protocol frame is skipped rather than fatal. Measured: this CLI writes occasional
//! non-protocol text to the same stream, and a probe against it had to skip such lines to work at all. Ending
//! every session on the connection because of one is the wrong trade, so they are counted and the count is
//! surfaced. Silently skipping them is what this avoids; treating them as fatal is what it also avoids.
//!
//! # Lifetime
//!
//! Sessions hold the connection and nothing else does. The reader task holds only the pieces it reads and
//! writes, so when the last session goes away the connection drops, the child is killed by its own
//! `kill_on_drop`, the stream ends, and the reader stops on its own. No shutdown handshake, and no way to
//! leave an `app-server` running with nobody attached.

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{ProviderError, ProviderId};
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};

use crate::codex::bound::{
    self, Answer, DECISION_DECLINE, DECISION_FIELD, HANDSHAKE, UNBOUND_REQUEST_CODE,
    UNBOUND_REQUEST_MESSAGE,
};
use crate::framing::jsonrpc::{self, Incoming, Pending, RequestId, WireError};
use crate::framing::{LineError, Lines};

/// How many frames one session's queue holds before the reader has to drop.
///
/// Measured: one short turn produced twenty notifications. A long turn streams fragments, and the supervisor
/// drains this in a loop, so the queue absorbs a burst rather than a backlog. At the tier bound of eight
/// sessions with a process, this is the depth at which a session that has stopped being read costs a bounded
/// amount and every other session keeps moving.
pub const INBOX_DEPTH: usize = 128;

/// How long runtrol waits for an answer to a protocol call.
///
/// Measured on this machine: the handshake answered in 4.2 seconds cold and starting a conversation in 6.4.
/// Ten times the slowest of those is generous against a loaded machine, and its purpose is that a daemon which
/// has stopped answering surfaces as a named failure instead of a session that waits forever.
///
/// **This is never applied to a turn.** A coding agent legitimately runs for an hour, and a turn's ending
/// arrives as a notification rather than as an answer, so nothing here bounds one.
pub const CALL_BUDGET_MS: u64 = 60_000;

/// What arrives for one session.
#[derive(Clone, Debug)]
pub enum Delivery {
    /// A notification the provider sent.
    Report {
        /// Which method.
        method: Box<str>,
        /// The provider's own parameters, unread.
        params: Option<Bytes>,
    },
    /// A question the provider asked, which runtrol answered on the spot.
    ///
    /// Relayed so the session can tell the operator. An approval that was declined and never mentioned is
    /// indistinguishable from the agent choosing not to act.
    Answered {
        /// Which method the provider asked.
        method: Box<str>,
        /// How runtrol answered it.
        how: Answer,
    },
}

/// One session's end of the connection.
pub struct Inbox {
    /// The conversation this belongs to, so dropping it can deregister.
    thread: Box<str>,
    /// Frames for this conversation.
    rx: mpsc::Receiver<Delivery>,
    /// How many frames were dropped because this queue was full.
    dropped: Arc<AtomicU64>,
    /// The routing table, so this route removes itself when the session ends.
    routes: Arc<Mutex<Routes>>,
}

impl Inbox {
    /// The next frame, or `None` once the connection is gone.
    ///
    /// # Abandoning this loses nothing
    ///
    /// The channel's receive is cancel safe, which is what lets one supervisor wait on every session at once.
    pub async fn next(&mut self) -> Option<Delivery> {
        self.rx.recv().await
    }

    /// How many frames have been dropped since this was last asked, and resets the count.
    ///
    /// Read by the session so it can say so. A dropped frame that nobody reports is output the operator
    /// silently never sees, which is the failure mode a bounded queue exists to make visible rather than to
    /// create.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }

    /// Stop following this conversation.
    ///
    /// Separate from dropping the value, because deregistering needs the routing table's lock and a drop
    /// cannot wait for one. Forgetting to call it is not a leak: the reader removes a route whose queue has
    /// been let go of, the next time it has something for it.
    pub async fn close(&self) {
        self.routes.lock().await.live.remove(&self.thread);
    }
}

/// Where each conversation's frames go.
#[derive(Debug, Default)]
pub struct Routes {
    /// One queue per conversation being followed.
    live: BTreeMap<Box<str>, Route>,
    /// Frames that named a conversation nobody is following.
    ///
    /// Counted rather than reported, because there is no session to report them to. See
    /// [`Connection::undeliverable`] for why this is expected to stay at zero.
    undeliverable: u64,
}

/// One conversation's queue, as the reader sees it.
#[derive(Debug)]
struct Route {
    /// Where frames go.
    tx: mpsc::Sender<Delivery>,
    /// How many were dropped because it was full.
    dropped: Arc<AtomicU64>,
}

/// One `app-server` process, shared by every session on it.
pub struct Connection {
    /// Which provider this is.
    provider: ProviderId,
    /// The child's input.
    ///
    /// Shared with the reader task, which writes answers to the questions the provider asks.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Questions runtrol is waiting on answers for.
    pending: Arc<Mutex<Pending>>,
    /// Where each conversation's frames go.
    routes: Arc<Mutex<Routes>>,
    /// Lines that were not protocol frames.
    unreadable: Arc<AtomicU64>,
    /// The child.
    ///
    /// Held so that dropping the connection stops the process. Never awaited on: the reader task notices the
    /// stream ending on its own.
    child: Mutex<Child>,
}

impl Connection {
    /// Start the daemon and complete its handshake.
    ///
    /// Returns once the provider has answered [`HANDSHAKE`], which is the point from which anything else may
    /// be asked of it. Measured: no follow-up notification is required, and a probe that sent only this and
    /// went straight to starting a conversation worked.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Spawn`] when the process cannot be started or has no streams,
    /// [`ProviderError::Protocol`] when the handshake is refused or unreadable, [`ProviderError::Timeout`]
    /// when it is not answered inside [`CALL_BUDGET_MS`].
    pub async fn start(
        provider: ProviderId,
        program: &Program,
        contained_by: &Containment,
        client: &str,
        version: &str,
    ) -> Result<Self, ProviderError> {
        let (child, stdin, stdout) = spawn(provider, program, contained_by)?;

        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(Pending::new()));
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let unreadable = Arc::new(AtomicU64::new(0));

        tokio::spawn(read_forever(
            Lines::new(stdout),
            Arc::clone(&stdin),
            Arc::clone(&pending),
            Arc::clone(&routes),
            Arc::clone(&unreadable),
        ));

        let connection = Self {
            provider,
            stdin,
            pending,
            routes,
            unreadable,
            child: Mutex::new(child),
        };

        // The one call that has to happen before anything else. Its answer is read for nothing: what matters
        // is that it came, because that is what says the daemon is speaking this protocol.
        connection
            .call(
                HANDSHAKE,
                &serde_json::json!({
                    "clientInfo": {"name": client, "version": version},
                }),
                "opening the connection",
            )
            .await?;

        Ok(connection)
    }

    /// Which provider this is.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    /// How many lines arrived that were not protocol frames.
    ///
    /// Measured: this CLI writes occasional non-protocol text to the same stream. Skipping those is necessary
    /// and counting them is what keeps the skipping from being silent.
    #[must_use]
    pub fn unreadable(&self) -> u64 {
        self.unreadable.load(Ordering::Relaxed)
    }

    /// How many frames named a conversation nobody was following.
    ///
    /// Expected to stay at zero, and the reason is structural rather than hopeful: every turn notification
    /// names a turn that only exists because runtrol asked for it, and runtrol can only ask after it has the
    /// conversation's identifier, which is the same answer that registers the route. The only frames that can
    /// precede registration are the ones announcing the conversation itself, and those repeat what that answer
    /// already carried.
    pub async fn undeliverable(&self) -> u64 {
        self.routes.lock().await.undeliverable
    }

    /// Follow a conversation.
    ///
    /// Registering the same conversation twice replaces the first queue, which is what a reattach is.
    pub async fn register(&self, thread: &str) -> Inbox {
        let (tx, rx) = mpsc::channel(INBOX_DEPTH);
        let dropped = Arc::new(AtomicU64::new(0));
        self.routes.lock().await.live.insert(
            thread.into(),
            Route {
                tx,
                dropped: Arc::clone(&dropped),
            },
        );
        Inbox {
            thread: thread.into(),
            rx,
            dropped,
            routes: Arc::clone(&self.routes),
        }
    }

    /// Ask the provider something and wait for its answer.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the frame cannot be written, too many answers are already outstanding,
    /// or the connection went away; [`ProviderError::NativeRefused`] when the provider answered with an error;
    /// [`ProviderError::Timeout`] when nothing came back inside [`CALL_BUDGET_MS`].
    pub async fn call<P: serde::Serialize>(
        &self,
        method: &str,
        params: &P,
        doing: &'static str,
    ) -> Result<Bytes, ProviderError> {
        let provider = self.provider;

        let (id, waiting) = {
            let mut pending = self.pending.lock().await;
            pending.issue().map_err(|full| ProviderError::Protocol {
                provider,
                doing,
                detail: full.to_string(),
            })?
        };

        let frame = jsonrpc::write_question(&id, method, params).map_err(|error| {
            ProviderError::Protocol {
                provider,
                doing,
                detail: error.to_string(),
            }
        })?;
        self.write_line(&frame, doing).await?;

        // The lock is released before waiting. Holding it would make one slow answer serialize every other
        // session's calls, which is the head-of-line problem this whole design exists to avoid.
        let answer = tokio::time::timeout(Duration::from_millis(CALL_BUDGET_MS), waiting).await;

        match answer {
            Err(_elapsed) => {
                // Nothing is left waiting for an answer that is not coming.
                self.pending.lock().await.resolve(
                    &id,
                    Err(WireError {
                        code: 0,
                        message: "runtrol stopped waiting".into(),
                        data: None,
                    }),
                );
                Err(ProviderError::Timeout {
                    provider,
                    doing,
                    waited_ms: CALL_BUDGET_MS,
                })
            }
            // The waiter was dropped without an answer, which happens when the reader stops.
            Ok(Err(_gone)) => Err(ProviderError::Protocol {
                provider,
                doing,
                detail: "the connection to the provider ended before it answered".to_owned(),
            }),
            Ok(Ok(Err(failure))) => Err(ProviderError::NativeRefused {
                provider,
                doing,
                detail: format!("{} (code {})", failure.message, failure.code),
            }),
            Ok(Ok(Ok(body))) => Ok(body),
        }
    }

    /// Write a frame exactly as it was given, expecting nothing back.
    ///
    /// The escape hatch that keeps runtrol a pipe: a surface can drive a provider feature this binary has
    /// never heard of. Never inspected and never rewritten.
    ///
    /// One consequence is worth stating rather than discovering. runtrol did not issue an identifier for this
    /// frame, so if it is a request, its answer arrives as an answer nobody asked for and is reported as such
    /// rather than delivered. A caller that needs an answer routed back should ask for a binding instead.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when the child's input cannot be written.
    pub async fn send_verbatim(&self, frame: &str) -> Result<(), ProviderError> {
        self.write_line(frame, "forwarding a frame").await
    }

    /// Write one line to the child's input.
    async fn write_line(&self, text: &str, doing: &'static str) -> Result<(), ProviderError> {
        write_line(&self.stdin, text)
            .await
            .map_err(|error| ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: error.to_string(),
            })
    }

    /// Stop the process now.
    ///
    /// Only for the case where the connection is being abandoned and the caller wants the exit reported.
    /// Dropping the connection does the same thing without a report.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Io`] when the process could not be stopped.
    pub async fn stop(&self) -> Result<(), ProviderError> {
        self.child
            .lock()
            .await
            .kill()
            .await
            .map_err(|source| ProviderError::Io {
                provider: self.provider,
                doing: "stopping the provider daemon",
                source,
            })
    }
}

/// Start the child and take its streams.
fn spawn(
    provider: ProviderId,
    program: &Program,
    contained_by: &Containment,
) -> Result<(Child, ChildStdin, ChildStdout), ProviderError> {
    let mut command = tokio::process::Command::new(program.path().as_std_path());
    command
        .args(program.leading())
        .args(["app-server", "--stdio"])
        // No working directory is set. Every conversation carries its own, so a daemon shared by sessions in
        // different places must not have one of theirs.
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Left alone rather than captured. What the CLI writes there is its own diagnostics, and a pipe nobody
        // reads fills up and blocks the process it belongs to.
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    contained_by.prepare(command.as_std_mut());

    let mut child = command.spawn().map_err(|source| ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source,
    })?;

    let missing = |what: &str| ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source: std::io::Error::other(format!("the child has no {what} stream")),
    };
    let stdin = child.stdin.take().ok_or_else(|| missing("input"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("output"))?;
    Ok((child, stdin, stdout))
}

/// Write one line, with its newline, in a single call.
async fn write_line(stdin: &Mutex<ChildStdin>, text: &str) -> Result<(), std::io::Error> {
    // The newline is what makes it a frame. Written with the body in one call so a frame cannot be half sent,
    // which on a shared stream would corrupt every session rather than one.
    let mut framed = String::with_capacity(text.len() + 1);
    framed.push_str(text);
    framed.push('\n');

    let mut stdin = stdin.lock().await;
    stdin.write_all(framed.as_bytes()).await?;
    stdin.flush().await
}

/// The only field the reader needs out of a notification's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Addressed<'line> {
    /// Which conversation it belongs to.
    #[serde(default)]
    thread_id: Option<&'line str>,
}

/// Which conversation a frame names, when it names one.
fn thread_of(params: Option<&Bytes>) -> Option<Box<str>> {
    let body = params?;
    let Ok(addressed) = serde_json::from_slice::<Addressed<'_>>(body) else {
        return None;
    };
    addressed.thread_id.map(Box::<str>::from)
}

/// Read the child's output until it ends, sorting every frame.
///
/// Holds no reference to the connection, which is what lets the last session going away stop the process and
/// then this task, rather than the two keeping each other alive.
async fn read_forever(
    mut lines: Lines<ChildStdout>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    routes: Arc<Mutex<Routes>>,
    unreadable: Arc<AtomicU64>,
) {
    let ended = loop {
        match lines.next().await {
            Ok(Some(line)) => match jsonrpc::read(&line) {
                Ok(incoming) => sort(incoming, &stdin, &pending, &routes).await,
                // Not a protocol frame. Measured: this CLI writes occasional non-protocol text to the same
                // stream, so ending every session on the connection over one line is the wrong trade. Counted
                // rather than ignored, and the count is what a session reports.
                Err(_) => {
                    unreadable.fetch_add(1, Ordering::Relaxed);
                }
            },
            Ok(None) => break "the provider's output ended",
            Err(LineError::TooLong { .. } | LineError::Poisoned) => {
                break "a frame went past the limit of the transport";
            }
            Err(LineError::Io { .. }) => break "reading from the provider failed",
        }
    };

    // Everyone waiting is told. A caller awaiting an answer that will never come is the worst shape a
    // swallowed failure takes, because nothing reports it and the session simply stops.
    pending.lock().await.abandon_all(0, ended);
    // Dropping every sender closes every session's queue, which is how each one learns the connection is gone.
    routes.lock().await.live.clear();
}

/// Hand one frame to whoever it belongs to.
async fn sort(
    incoming: Incoming,
    stdin: &Mutex<ChildStdin>,
    pending: &Mutex<Pending>,
    routes: &Mutex<Routes>,
) {
    match incoming {
        Incoming::Answer { id, outcome } => {
            // An answer nobody asked for, or whose caller left, is reported by `resolve` and is not an error
            // here: it happens whenever a session closes with a question outstanding.
            let _delivered = pending.lock().await.resolve(&id, outcome);
        }

        Incoming::Question { id, method, params } => {
            let how = answer_question(stdin, &id, &method).await;
            if let Some(thread) = thread_of(params.as_ref()) {
                deliver(
                    routes,
                    &thread,
                    Delivery::Answered {
                        method,
                        how: how.unwrap_or(Answer::Refuse),
                    },
                )
                .await;
            }
        }

        Incoming::Report { method, params } => match bound::is_per_thread(&method) {
            // Account state. True of every session on this connection at once, so it goes to all of them
            // rather than being attached to whichever one happened to be first.
            Some(false) => broadcast(routes, &Delivery::Report { method, params }).await,
            _ => match thread_of(params.as_ref()) {
                Some(thread) => deliver(routes, &thread, Delivery::Report { method, params }).await,
                // A notification that names no conversation and is not one of the account-wide ones. There is
                // no session it belongs to, so it is counted where a test can see it.
                None => routes.lock().await.undeliverable += 1,
            },
        },
    }
}

/// Answer a question the provider asked.
///
/// Always answers. A question left open stalls the daemon, and the daemon is every session at once.
async fn answer_question(
    stdin: &Mutex<ChildStdin>,
    id: &RequestId,
    method: &str,
) -> Option<Answer> {
    let how = bound::answer_for(method);
    let frame = match how {
        // A decline is a value the protocol has: the provider carries on knowing the answer was no.
        Some(Answer::Decline) => {
            jsonrpc::write_answer(id, &serde_json::json!({DECISION_FIELD: DECISION_DECLINE}))
        }
        // No honest answer exists, so the answer is that runtrol will not serve it.
        Some(Answer::Refuse) | None => Ok(jsonrpc::write_error(
            id,
            UNBOUND_REQUEST_CODE,
            UNBOUND_REQUEST_MESSAGE,
        )),
    };

    let Ok(frame) = frame else {
        // The answer could not even be written. Nothing further can be done here, and the daemon will see the
        // connection end when the last session drops it.
        return None;
    };
    match write_line(stdin, &frame).await {
        Ok(()) => how.or(Some(Answer::Refuse)),
        // The child's input is gone, which means the connection is ending anyway.
        Err(_) => None,
    }
}

/// Put a frame in one conversation's queue.
async fn deliver(routes: &Mutex<Routes>, thread: &str, delivery: Delivery) {
    let mut table = routes.lock().await;
    let Some(route) = table.live.get(thread) else {
        table.undeliverable += 1;
        return;
    };
    // Never waits. Making the reader wait for one full queue would stop every other session on the
    // connection, so the frame is dropped and counted, and the session reports the count.
    match route.tx.try_send(delivery) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            route.dropped.fetch_add(1, Ordering::Relaxed);
        }
        // The session let go of its queue without deregistering. Removing the route is what stops the same
        // frame being counted forever.
        Err(mpsc::error::TrySendError::Closed(_)) => {
            table.live.remove(thread);
        }
    }
}

/// Put a frame in every conversation's queue.
async fn broadcast(routes: &Mutex<Routes>, delivery: &Delivery) {
    let mut table = routes.lock().await;
    let mut gone: Vec<Box<str>> = Vec::new();
    for (thread, route) in &table.live {
        match route.tx.try_send(delivery.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                route.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => gone.push(thread.clone()),
        }
    }
    for thread in gone {
        table.live.remove(&thread);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    async fn a_route(routes: &Arc<Mutex<Routes>>, thread: &str) -> Inbox {
        let (tx, rx) = mpsc::channel(INBOX_DEPTH);
        let dropped = Arc::new(AtomicU64::new(0));
        routes.lock().await.live.insert(
            thread.into(),
            Route {
                tx,
                dropped: Arc::clone(&dropped),
            },
        );
        Inbox {
            thread: thread.into(),
            rx,
            dropped,
            routes: Arc::clone(routes),
        }
    }

    #[test]
    fn a_frame_is_sorted_by_the_conversation_it_names() {
        // The property the whole file rests on: one stream, many sessions, and no guessing about which.
        assert_eq!(
            thread_of(Some(&params(r#"{"threadId":"t-1","turnId":"x"}"#))).as_deref(),
            Some("t-1")
        );
        assert_eq!(thread_of(Some(&params(r#"{"turnId":"x"}"#))), None);
        assert_eq!(thread_of(None), None);
        // Unreadable parameters name nothing rather than taking the reader down with them.
        assert_eq!(thread_of(Some(&params("not json"))), None);
    }

    #[tokio::test]
    async fn a_frame_reaches_only_the_session_it_belongs_to() {
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let mut one = a_route(&routes, "t-1").await;
        let mut two = a_route(&routes, "t-2").await;

        deliver(
            &routes,
            "t-1",
            Delivery::Report {
                method: "turn/completed".into(),
                params: None,
            },
        )
        .await;

        match one.next().await {
            Some(Delivery::Report { method, .. }) => assert_eq!(&*method, "turn/completed"),
            other => panic!("the addressed session got {other:?}"),
        }
        // The other session must not see it. A supervisor that received another conversation's ending would
        // end the wrong turn.
        assert!(two.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn account_state_reaches_every_session_at_once() {
        // It names no conversation because it is true of all of them. Attaching it to one would leave every
        // other session showing a quota it cannot see.
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let mut one = a_route(&routes, "t-1").await;
        let mut two = a_route(&routes, "t-2").await;

        broadcast(
            &routes,
            &Delivery::Report {
                method: "account/rateLimits/updated".into(),
                params: Some(params(r#"{"rateLimits":{}}"#)),
            },
        )
        .await;

        for inbox in [&mut one, &mut two] {
            match inbox.next().await {
                Some(Delivery::Report { method, .. }) => {
                    assert_eq!(&*method, "account/rateLimits/updated");
                }
                other => panic!("a session missed account state: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_full_queue_drops_and_counts_rather_than_stalling_every_other_session() {
        // The head-of-line rule. Making the reader wait for one session would stop the whole connection, so a
        // frame is dropped, counted, and reported by the session that lost it.
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let inbox = a_route(&routes, "t-1").await;

        for _ in 0..(INBOX_DEPTH + 5) {
            deliver(
                &routes,
                "t-1",
                Delivery::Report {
                    method: "item/agentMessage/delta".into(),
                    params: None,
                },
            )
            .await;
        }

        assert_eq!(inbox.dropped(), 5, "what was lost has to be countable");
        assert_eq!(
            inbox.dropped(),
            0,
            "and reading the count clears it, so it is reported once"
        );
    }

    #[tokio::test]
    async fn a_frame_for_a_conversation_nobody_follows_is_counted() {
        // There is no session to report this to, so it is counted where a test can see it rather than
        // vanishing. Expected to stay at zero in practice, for the structural reason on `undeliverable`.
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        deliver(
            &routes,
            "nobody-is-following-this",
            Delivery::Report {
                method: "turn/completed".into(),
                params: None,
            },
        )
        .await;
        assert_eq!(routes.lock().await.undeliverable, 1);
    }

    #[tokio::test]
    async fn a_session_that_let_go_of_its_queue_stops_being_a_route() {
        // Otherwise the same frame is counted as dropped forever, and the table grows a dead entry per
        // session that ever existed.
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let inbox = a_route(&routes, "t-1").await;
        drop(inbox);

        deliver(
            &routes,
            "t-1",
            Delivery::Report {
                method: "turn/completed".into(),
                params: None,
            },
        )
        .await;
        assert!(routes.lock().await.live.is_empty());
    }

    #[tokio::test]
    async fn closing_a_session_removes_its_route() {
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(Routes::default()));
        let inbox = a_route(&routes, "t-1").await;
        assert_eq!(routes.lock().await.live.len(), 1);
        inbox.close().await;
        assert!(routes.lock().await.live.is_empty());
    }

    #[test]
    fn the_budget_is_never_applied_to_a_turn() {
        // A coding agent legitimately runs for an hour. The budget covers protocol calls, whose slowest
        // measured answer was 6.4 seconds, and a turn's ending arrives as a notification rather than as an
        // answer, so nothing bounds one.
        const { assert!(CALL_BUDGET_MS >= 60_000) };
        assert!(
            !bound::CALLS
                .iter()
                .any(|call| call.method == bound::TERMINAL),
            "the ending is not something runtrol calls and waits for"
        );
    }
}
