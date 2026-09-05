//! The admitted command boundary: exact ownership, wakeups, deadlines, and bounded wait leases.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtrol_courier::wire::{Answer, Request, SESSION_WAIT_SLOTS};
use runtrol_courier::{
    BoundedUtf8, CallEnvelope, CallRef, DeliveryState, Limits, ManagedSessionId, MessageId,
    UnixMillis,
};
use runtrol_provider::TerminalId;

use super::{CourierGate, gate, session_id};

struct Fleet {
    gate: CourierGate,
    alpha: ManagedSessionId,
    bravo: ManagedSessionId,
    charlie: ManagedSessionId,
    alpha_terminal: TerminalId,
}

async fn start(gate: &CourierGate) -> (TerminalId, ManagedSessionId) {
    let terminal = TerminalId::now();
    let minted = gate.mint(terminal).expect("mint test authority");
    gate.launch(minted, || Ok::<_, ()>(((), None)))
        .await
        .expect("register a mailbox without launching a process");
    (terminal, session_id(terminal))
}

async fn fleet() -> Fleet {
    let gate = gate();
    let (alpha_terminal, alpha) = start(&gate).await;
    let (_bravo_terminal, bravo) = start(&gate).await;
    let (_charlie_terminal, charlie) = start(&gate).await;
    Fleet {
        gate,
        alpha,
        bravo,
        charlie,
        alpha_terminal,
    }
}

fn now() -> UnixMillis {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after the epoch")
        .as_millis();
    UnixMillis(u64::try_from(millis).expect("test clock fits in milliseconds"))
}

fn body(text: &str) -> BoundedUtf8 {
    BoundedUtf8::new(text.to_owned(), Limits::INITIAL.body_bytes).expect("bounded fixture body")
}

fn reference(ask: &CallEnvelope) -> CallRef {
    CallRef {
        call_id: ask.call_id,
        ask: ask.message_id,
    }
}

async fn pending_once<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|context| {
        assert!(
            future.as_mut().poll(context).is_pending(),
            "the command reached its pending wait"
        );
        Poll::Ready(())
    })
    .await;
}

async fn receive(gate: &CourierGate, session: ManagedSessionId) -> Answer {
    gate.command(
        session,
        Request::Receive {
            source: None,
            call: None,
            timeout_ms: 0,
        },
    )
    .await
}

#[tokio::test]
async fn send_and_ask_cannot_spoof_another_admitted_sessions_source() {
    let Fleet {
        gate,
        alpha,
        bravo,
        charlie,
        ..
    } = fleet().await;
    for asking in [false, true] {
        let envelope = if asking {
            CallEnvelope::ask(alpha, bravo, body("private question"), now().plus(5_000))
        } else {
            CallEnvelope::tell(alpha, bravo, body("private message"), now().plus(5_000))
        };
        let message_id = envelope.message_id;
        let request = if asking {
            Request::Ask { envelope }
        } else {
            Request::Send { envelope }
        };
        let answer = gate.command(charlie, request).await;
        assert!(matches!(answer, Answer::Refused { .. }));
        assert!(!format!("{answer:?}").contains("private"));
        let state = gate.state.lock().await;
        assert_eq!(state.courier.waiting(bravo), Some(0));
        assert_eq!(state.courier.active_calls(), 0);
        assert_eq!(state.courier.charged_bytes(), 0);
        assert_eq!(state.courier.state_of(message_id), None);
    }
}

#[tokio::test]
async fn another_inbox_consuming_a_reply_wakes_an_exact_wait_for_the_ended_call() {
    let Fleet {
        gate,
        alpha,
        bravo,
        charlie,
        ..
    } = fleet().await;
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), now().plus(5_000));
    assert!(matches!(
        gate.command(
            alpha,
            Request::Send {
                envelope: ask.clone()
            }
        )
        .await,
        Answer::Accepted { .. }
    ));
    assert!(matches!(
        receive(&gate, bravo).await,
        Answer::Received { envelope: Some(_) }
    ));
    assert!(matches!(
        gate.command(
            bravo,
            Request::Reply {
                message: ask.message_id,
                message_id: MessageId::now(),
                body: body("answer"),
            }
        )
        .await,
        Answer::Accepted { .. }
    ));

    // This filter cannot consume the queued reply, but the call still belongs to alpha until another inbox reads it.
    let mut waiting = Box::pin(gate.command(
        alpha,
        Request::Receive {
            source: Some(charlie),
            call: Some(reference(&ask)),
            timeout_ms: 5_000,
        },
    ));
    pending_once(waiting.as_mut()).await;
    let Answer::Received {
        envelope: Some(reply),
    } = receive(&gate, alpha).await
    else {
        panic!("the other inbox consumes the reply");
    };
    assert_eq!(reply.reply_to, Some(ask.message_id));
    let answer = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .expect("consumption wakes the exact waiter before its five-second deadline");
    assert!(matches!(answer, Answer::Refused { .. }));
    assert_eq!(gate.state.lock().await.courier.active_calls(), 0);
}

#[tokio::test]
async fn a_positive_wait_does_not_consume_mail_that_arrives_after_its_timeout() {
    let Fleet {
        gate, alpha, bravo, ..
    } = fleet().await;
    let mut waiting = Box::pin(gate.command(
        bravo,
        Request::Receive {
            source: None,
            call: None,
            timeout_ms: 20,
        },
    ));
    pending_once(waiting.as_mut()).await;
    // Keep the waiter unpolled while its monotonic deadline passes. Mail exists by the time it is polled again.
    tokio::time::sleep(Duration::from_millis(40)).await;
    let mail = CallEnvelope::tell(alpha, bravo, body("arrived too late"), now().plus(5_000));
    assert!(matches!(
        gate.command(
            alpha,
            Request::Send {
                envelope: mail.clone()
            }
        )
        .await,
        Answer::Accepted { .. }
    ));
    let answer = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .expect("the expired wait finishes");
    assert!(matches!(answer, Answer::Received { envelope: None }));
    let Answer::Received {
        envelope: Some(received),
    } = receive(&gate, bravo).await
    else {
        panic!("the later inbox still owns the mail");
    };
    assert_eq!(received.message_id, mail.message_id);
}

#[tokio::test]
async fn session_wait_slots_are_bounded_independent_and_released_on_drop() {
    let Fleet {
        gate,
        alpha,
        bravo,
        alpha_terminal,
        ..
    } = fleet().await;
    let mut held = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        held.push(gate.wait_slot(alpha).await.expect("one bounded wait slot"));
    }
    assert!(gate.wait_slot(alpha).await.is_none());
    let other = gate
        .wait_slot(bravo)
        .await
        .expect("another session has its own allowance");
    let mail = CallEnvelope::tell(bravo, alpha, body("wake"), now().plus(5_000));
    assert!(matches!(
        gate.command(bravo, Request::Send { envelope: mail }).await,
        Answer::Accepted { .. }
    ));
    drop(held.pop().expect("release exactly one slot"));
    let replacement = gate
        .wait_slot(alpha)
        .await
        .expect("the released slot is reusable");
    assert!(gate.wait_slot(alpha).await.is_none());
    drop(replacement);
    drop(held);
    drop(other);
    let mut restored = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        restored.push(
            gate.wait_slot(alpha)
                .await
                .expect("all wait slots were returned"),
        );
    }
    gate.forget(alpha_terminal).await;
    assert!(
        gate.wait_slot(alpha).await.is_none(),
        "an ended session acquires no fresh lease"
    );
    drop(restored);
}

#[tokio::test]
async fn cancelling_an_owned_ask_wait_releases_queued_and_received_call_state() {
    for received in [false, true] {
        let Fleet {
            gate, alpha, bravo, ..
        } = fleet().await;
        let ask = CallEnvelope::ask(alpha, bravo, body("question"), now().plus(5_000));
        let slot = gate
            .wait_slot(alpha)
            .await
            .expect("the connection's wait lease");
        let mut waiting = Box::pin(gate.command(
            alpha,
            Request::Ask {
                envelope: ask.clone(),
            },
        ));
        pending_once(waiting.as_mut()).await;
        assert_eq!(gate.state.lock().await.courier.active_calls(), 1);
        if received {
            assert!(matches!(
                receive(&gate, bravo).await,
                Answer::Received { envelope: Some(_) }
            ));
        }
        drop(waiting);
        // The serving connection performs this exact cleanup when its peer leaves or sends a second frame.
        gate.abandon(alpha, reference(&ask)).await;
        drop(slot);
        let state = gate.state.lock().await;
        assert_eq!(state.courier.active_calls(), 0);
        assert_eq!(state.courier.charged_bytes(), 0);
        assert_eq!(state.courier.waiting(bravo), Some(0));
        assert_eq!(
            state.courier.state_of(ask.message_id),
            Some(DeliveryState::Cancelled)
        );
        assert_eq!(
            state
                .sessions
                .get(&alpha)
                .expect("live session")
                .waits
                .available_permits(),
            SESSION_WAIT_SLOTS
        );
    }
}

#[tokio::test]
async fn ending_a_session_wakes_its_wait_and_releases_its_queued_ask() {
    let Fleet {
        gate,
        alpha,
        bravo,
        alpha_terminal,
        ..
    } = fleet().await;
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), now().plus(5_000));
    let mut waiting = Box::pin(gate.command(
        alpha,
        Request::Ask {
            envelope: ask.clone(),
        },
    ));
    pending_once(waiting.as_mut()).await;
    gate.forget(alpha_terminal).await;
    let answer = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .expect("session exit wakes the waiter");
    assert!(matches!(answer, Answer::Refused { .. }));
    let state = gate.state.lock().await;
    assert_eq!(state.courier.active_calls(), 0);
    assert_eq!(state.courier.charged_bytes(), 0);
    assert_eq!(state.courier.waiting(bravo), Some(0));
    assert_eq!(
        state.courier.state_of(ask.message_id),
        Some(DeliveryState::Cancelled)
    );
}

#[tokio::test]
async fn a_refused_duplicate_ask_does_not_acquire_cleanup_of_the_original_call() {
    let Fleet {
        gate, alpha, bravo, ..
    } = fleet().await;
    let ask = CallEnvelope::ask(alpha, bravo, body("original"), now().plus(5000));
    assert!(matches!(
        gate.command(
            alpha,
            Request::Send {
                envelope: ask.clone()
            }
        )
        .await,
        Answer::Accepted { .. }
    ));
    let mut pending = None;
    let answer = gate
        .command_owned(
            alpha,
            Request::Ask {
                envelope: ask.clone(),
            },
            &mut pending,
        )
        .await;
    assert!(matches!(answer, Answer::Refused { .. }));
    assert!(pending.is_none());
    let state = gate.state.lock().await;
    assert_eq!(state.courier.active_calls(), 1);
    assert_eq!(state.courier.charged_bytes(), ask.body.len());
}
