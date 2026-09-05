//! The courier listener: accept a connection, hear one hello within a deadline, then serve the admitted session.
//!
//! Everything before admission fails closed and says nothing useful to the peer: a frame that is too large, a
//! first frame that is not a hello, a hello the gate denies, and silence past the deadline all end with the
//! connection closed and, at most, a refusal the peer cannot learn from. The daemon's error stream carries the
//! reason, without the token.

use std::sync::Arc;
use std::time::Duration;

use runtrol_childproc::Containment;
use runtrol_courier::ManagedSessionId;
use runtrol_courier::wire::{
    Answer, COMMAND_SLOTS, HelloAnswer, Invocation, MAX_FRAME_BYTES, Request, WAIT_SLOTS,
};
use runtrol_ipc::transport::PeerProcess;
use runtrol_ipc::{Connection, Listener, TransportError};
use runtrol_provider::ProcessIdentity;

use super::{Admitted, CourierGate};

/// How long a connection has to say hello before it is dropped unadmitted.
pub(crate) const HELLO_WAIT: Duration = Duration::from_secs(5);

/// Connections that may be waiting to say hello at once. Beyond this, accept waits: a process that opens
/// connections and says nothing holds at most this many slots for [`HELLO_WAIT`], never the listener.
const GREETING_SLOTS: usize = 8;

/// How long to wait after the listener refuses an accept before asking again.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// Serve the courier endpoint until the task is dropped with its generation.
pub(crate) async fn serve(
    gate: Arc<CourierGate>,
    containment: Arc<Containment>,
    mut listener: Listener,
    hello_wait: Duration,
) {
    let slots = Arc::new(tokio::sync::Semaphore::new(GREETING_SLOTS));
    let commands = Arc::new(tokio::sync::Semaphore::new(COMMAND_SLOTS));
    let waits = Arc::new(tokio::sync::Semaphore::new(WAIT_SLOTS));
    let mut tasks = tokio::task::JoinSet::new();
    let expiry = Arc::clone(&gate);
    tasks.spawn(async move {
        expiry.expire().await;
    });
    loop {
        while let Some(result) = tasks.try_join_next() {
            if result.is_err() {
                report_denied(None, "a courier connection task ended unexpectedly");
            }
        }
        // The semaphore is never closed, so this cannot fail; the honest shape for a closed one is to stop.
        let Ok(slot) = Arc::clone(&slots).acquire_owned().await else {
            return;
        };
        let connection = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => {
                report_accept_failure(&error);
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        let gate = Arc::clone(&gate);
        let containment = Arc::clone(&containment);
        let commands = Arc::clone(&commands);
        let waits = Arc::clone(&waits);
        tasks.spawn(async move {
            greet(
                &gate,
                &containment,
                connection,
                hello_wait,
                slot,
                commands,
                waits,
            )
            .await;
        });
    }
}

async fn greet(
    gate: &CourierGate,
    containment: &Containment,
    mut connection: Connection,
    hello_wait: Duration,
    greeting: tokio::sync::OwnedSemaphorePermit,
    commands: Arc<tokio::sync::Semaphore>,
    waits: Arc<tokio::sync::Semaphore>,
) {
    // Silence past the deadline, a peer that left, or a broken read: nothing was admitted, and there is no one
    // to tell. The connection ends here.
    let frame =
        match tokio::time::timeout(hello_wait, connection.recv_bounded(MAX_FRAME_BYTES)).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Err(error)) => {
                report_denied(
                    connection.peer_process().map(PeerProcess::identity),
                    &error.to_string(),
                );
                return;
            }
            Ok(Ok(None)) | Err(_) => return,
        };
    let peer = connection.peer_process().map(PeerProcess::identity);
    let invocation: Invocation = match serde_json::from_slice(&frame) {
        Ok(invocation) => invocation,
        Err(_not_a_hello) => {
            report_denied(peer, "its first frame is not a hello");
            refuse(&mut connection).await;
            return;
        }
    };
    drop(frame);
    match gate.admit(containment, peer, &invocation.hello).await {
        Ok(admitted) => {
            let session = admitted.session;
            drop(greeting);
            let Some(request) = invocation.request else {
                welcome(&mut connection, session).await;
                return;
            };
            let waiting = matches!(&request, Request::Ask { .. } | Request::RoomAsk { .. })
                || matches!(&request, Request::Receive { timeout_ms, .. } if *timeout_ms > 0);
            let _session_slot = if waiting {
                let Some(slot) = gate.wait_slot(admitted).await else {
                    refuse(&mut connection).await;
                    return;
                };
                Some(slot)
            } else {
                None
            };
            let Ok(_slot) = if waiting { waits } else { commands }.try_acquire_owned() else {
                refuse(&mut connection).await;
                return;
            };
            if welcome(&mut connection, session).await {
                command(gate, admitted, &mut connection, request).await;
            }
        }
        Err(denied) => {
            report_denied(peer, &denied.to_string());
            refuse(&mut connection).await;
        }
    }
}

/// Tell the peer it is in. A failed answer ends the connection just like a delivered one.
async fn welcome(connection: &mut Connection, session: ManagedSessionId) -> bool {
    let Ok(bytes) = serde_json::to_vec(&HelloAnswer::Welcome { session }) else {
        return false;
    };
    // No command is pending and this connection ends now, including when the peer has already left.
    matches!(
        tokio::time::timeout(HELLO_WAIT, connection.send(&bytes)).await,
        Ok(Ok(()))
    )
}

async fn command(
    gate: &CourierGate,
    admitted: Admitted,
    connection: &mut Connection,
    request: Request,
) {
    let mut call = None;
    let answer = tokio::select! {
        answer = gate.command_owned(admitted, request, &mut call) => Some(answer),
        // One command per connection. Closing it or sending another frame cancels its pending wait.
        _closed = connection.recv_bounded(MAX_FRAME_BYTES) => None,
    };
    if let Some(call) = call
        && !matches!(&answer, Some(Answer::Received { envelope: Some(_) }))
    {
        gate.abandon(admitted.session, call).await;
    }
    if let Some(answer) = answer
        && let Ok(bytes) = serde_json::to_vec(&answer)
    {
        // Receive is an at-most-once handoff. A peer that leaves after consuming it cannot roll delivery back.
        drop(tokio::time::timeout(HELLO_WAIT, connection.send(&bytes)).await);
    }
}

async fn refuse(connection: &mut Connection) {
    // The peer learns only that it was refused. A send that fails changes nothing: the connection ends
    // either way, which is the outcome a refusal is.
    if let Ok(bytes) = serde_json::to_vec(&HelloAnswer::Refused) {
        // ok: the peer is being refused and the connection ends here; whether the refusal notice reaches it
        // changes nothing, and there is no session to promote a write failure into.
        drop(tokio::time::timeout(HELLO_WAIT, connection.send(&bytes)).await);
    }
}

/// Said on the daemon's error stream, the only surface a refused connection has. Never the token.
#[expect(
    clippy::print_stderr,
    reason = "the daemon's error stream is the existing operational channel for a background refusal"
)]
fn report_denied(peer: Option<ProcessIdentity>, why: &str) {
    match peer {
        Some(peer) => eprintln!("runtrol: courier refused process {}: {why}", peer.pid()),
        None => eprintln!("runtrol: courier refused an unidentified process: {why}"),
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the daemon's error stream is the existing operational channel for a listener that cannot accept"
)]
fn report_accept_failure(error: &TransportError) {
    eprintln!("runtrol: the courier endpoint could not accept a connection: {error}");
}
