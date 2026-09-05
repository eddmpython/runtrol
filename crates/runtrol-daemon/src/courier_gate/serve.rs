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
use runtrol_courier::wire::{Hello, HelloAnswer, MAX_FRAME_BYTES};
use runtrol_ipc::transport::PeerProcess;
use runtrol_ipc::{Connection, Listener, TransportError};
use runtrol_provider::ProcessIdentity;

use super::CourierGate;

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
    loop {
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
        tokio::spawn(async move {
            greet(&gate, &containment, connection, hello_wait).await;
            // The greeting slot is held for exactly this connection's greeting, then released for the next.
            drop(slot);
        });
    }
}

async fn greet(
    gate: &CourierGate,
    containment: &Containment,
    mut connection: Connection,
    hello_wait: Duration,
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
    let hello: Hello = match serde_json::from_slice(&frame) {
        Ok(hello) => hello,
        Err(_not_a_hello) => {
            report_denied(peer, "its first frame is not a hello");
            refuse(&mut connection).await;
            return;
        }
    };
    match gate.admit(containment, peer, &hello).await {
        Ok(session) => {
            // Hello is the complete command at this stage. End its connection after the answer so an
            // admitted peer cannot hold a greeting slot forever by leaving the connection idle.
            welcome(&mut connection, session).await;
        }
        Err(denied) => {
            report_denied(peer, &denied.to_string());
            refuse(&mut connection).await;
        }
    }
}

/// Tell the peer it is in. A failed answer ends the connection just like a delivered one.
async fn welcome(connection: &mut Connection, session: ManagedSessionId) {
    let Ok(bytes) = serde_json::to_vec(&HelloAnswer::Welcome { session }) else {
        return;
    };
    // No command is pending and this connection ends now, including when the peer has already left.
    drop(connection.send(&bytes).await);
}

async fn refuse(connection: &mut Connection) {
    // The peer learns only that it was refused. A send that fails changes nothing: the connection ends
    // either way, which is the outcome a refusal is.
    if let Ok(bytes) = serde_json::to_vec(&HelloAnswer::Refused) {
        // ok: the peer is being refused and the connection ends here; whether the refusal notice reaches it
        // changes nothing, and there is no session to promote a write failure into.
        drop(connection.send(&bytes).await);
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
