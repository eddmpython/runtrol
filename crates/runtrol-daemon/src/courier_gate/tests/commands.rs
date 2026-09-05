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

pub(super) async fn admission(
    gate: &CourierGate,
    session: ManagedSessionId,
) -> super::super::Admitted {
    gate.state
        .lock()
        .await
        .sessions
        .get(&session)
        .expect("registered session")
        .admission(session)
}

pub(super) struct Fleet {
    pub(super) gate: CourierGate,
    pub(super) alpha: ManagedSessionId,
    pub(super) bravo: ManagedSessionId,
    pub(super) charlie: ManagedSessionId,
    pub(super) alpha_terminal: TerminalId,
}

async fn start(gate: &CourierGate) -> (TerminalId, ManagedSessionId) {
    let terminal = TerminalId::now();
    let minted = gate.mint(terminal).expect("mint test authority");
    gate.launch(minted, || Ok::<_, ()>(((), None)))
        .await
        .expect("register a mailbox without launching a process");
    gate.set_dialogue(terminal, true)
        .await
        .expect("explicitly arm test session");
    (terminal, session_id(terminal))
}

pub(super) async fn fleet() -> Fleet {
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

pub(super) fn now() -> UnixMillis {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after the epoch")
        .as_millis();
    UnixMillis(u64::try_from(millis).expect("test clock fits in milliseconds"))
}

pub(super) fn body(text: &str) -> BoundedUtf8 {
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
async fn launch_is_disabled_and_disabling_releases_calls_and_mail() {
    let gate = gate();
    let terminal = TerminalId::now();
    let session = session_id(terminal);
    gate.launch(gate.mint(terminal).expect("mint"), || {
        Ok::<_, ()>(((), None))
    })
    .await
    .expect("launch");
    assert!(!gate.dialogue_enabled(terminal).await);
    assert!(matches!(
        gate.command(session, Request::List { after: None }).await,
        Answer::Refused { .. }
    ));
    let (peer_terminal, peer) = start(&gate).await;
    let ask = CallEnvelope::ask(peer, session, body("question"), now().plus(5_000));
    assert!(matches!(
        gate.command(
            peer,
            Request::Send {
                envelope: ask.clone()
            }
        )
        .await,
        Answer::Refused { .. }
    ));
    gate.set_dialogue(terminal, true).await.expect("arm");
    assert!(matches!(
        gate.command(peer, Request::Send { envelope: ask }).await,
        Answer::Accepted { .. }
    ));
    gate.set_dialogue(terminal, false).await.expect("disarm");
    let state = gate.state.lock().await;
    assert_eq!(state.courier.charged_bytes(), 0);
    assert_eq!(state.courier.active_calls(), 0);
    drop(state);
    gate.set_dialogue(terminal, true).await.expect("rearm");
    assert!(matches!(
        receive(&gate, session).await,
        Answer::Received { envelope: None }
    ));
    gate.forget(terminal).await;
    assert!(gate.set_dialogue(terminal, true).await.is_err());
    assert!(gate.dialogue_enabled(peer_terminal).await);
}

#[tokio::test]
async fn an_old_wait_cannot_take_mail_after_disable_and_reenable() {
    let Fleet {
        gate,
        alpha,
        bravo,
        alpha_terminal,
        ..
    } = fleet().await;
    let mut waiting = Box::pin(gate.command(
        alpha,
        Request::Receive {
            source: None,
            call: None,
            timeout_ms: 5_000,
        },
    ));
    pending_once(waiting.as_mut()).await;
    gate.set_dialogue(alpha_terminal, false)
        .await
        .expect("disarm");
    gate.set_dialogue(alpha_terminal, true)
        .await
        .expect("rearm");
    let message = CallEnvelope::tell(bravo, alpha, body("new activation"), now().plus(5_000));
    assert!(matches!(
        gate.command(bravo, Request::Send { envelope: message })
            .await,
        Answer::Accepted { .. }
    ));
    assert!(matches!(waiting.await, Answer::Refused { .. }));
    assert!(matches!(
        receive(&gate, alpha).await,
        Answer::Received { envelope: Some(_) }
    ));
}

#[tokio::test]
async fn stale_cleanup_cannot_cancel_a_new_activation_reusing_the_same_call() {
    let Fleet {
        gate,
        alpha,
        bravo,
        alpha_terminal,
        ..
    } = fleet().await;
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), now().plus(5_000));
    let mut pending = None;
    let mut waiting = Box::pin(gate.command_owned(
        admission(&gate, alpha).await,
        Request::Ask {
            envelope: ask.clone(),
        },
        &mut pending,
    ));
    pending_once(waiting.as_mut()).await;
    drop(waiting);
    let stale = pending.expect("owned call");
    gate.set_dialogue(alpha_terminal, false)
        .await
        .expect("disarm");
    gate.set_dialogue(alpha_terminal, true)
        .await
        .expect("rearm");
    // Replay memory is bounded. An old command may remain unpolled while later traffic rotates that memory.
    for _ in 0..Limits::INITIAL.remembered_messages {
        let note = CallEnvelope::tell(alpha, bravo, body("rotate"), now().plus(5_000));
        assert!(matches!(
            gate.command(alpha, Request::Send { envelope: note }).await,
            Answer::Accepted { .. }
        ));
        assert!(matches!(
            receive(&gate, bravo).await,
            Answer::Received { envelope: Some(_) }
        ));
    }
    assert!(matches!(
        gate.command(alpha, Request::Send { envelope: ask }).await,
        Answer::Accepted { .. }
    ));
    gate.abandon(alpha, stale).await;
    assert_eq!(gate.state.lock().await.courier.active_calls(), 1);
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
    let alpha_admitted = admission(&gate, alpha).await;
    let bravo_admitted = admission(&gate, bravo).await;
    let mut held = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        held.push(
            gate.wait_slot(alpha_admitted)
                .await
                .expect("one bounded wait slot"),
        );
    }
    assert!(gate.wait_slot(alpha_admitted).await.is_none());
    let other = gate
        .wait_slot(bravo_admitted)
        .await
        .expect("another session has its own allowance");
    let mail = CallEnvelope::tell(bravo, alpha, body("wake"), now().plus(5_000));
    assert!(matches!(
        gate.command(bravo, Request::Send { envelope: mail }).await,
        Answer::Accepted { .. }
    ));
    drop(held.pop().expect("release exactly one slot"));
    let replacement = gate
        .wait_slot(alpha_admitted)
        .await
        .expect("the released slot is reusable");
    assert!(gate.wait_slot(alpha_admitted).await.is_none());
    drop(replacement);
    drop(held);
    drop(other);
    let mut restored = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        restored.push(
            gate.wait_slot(alpha_admitted)
                .await
                .expect("all wait slots were returned"),
        );
    }
    gate.forget(alpha_terminal).await;
    assert!(
        gate.wait_slot(alpha_admitted).await.is_none(),
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
            .wait_slot(admission(&gate, alpha).await)
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
        let activation = gate
            .state
            .lock()
            .await
            .sessions
            .get(&alpha)
            .expect("registered")
            .activation;
        gate.abandon(
            alpha,
            super::super::PendingCall {
                activation,
                call: reference(&ask),
            },
        )
        .await;
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
            admission(&gate, alpha).await,
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

#[tokio::test]
async fn commands_delayed_after_hello_cannot_enter_a_later_activation() {
    let Fleet {
        gate,
        alpha,
        bravo,
        alpha_terminal,
        ..
    } = fleet().await;
    let stale = admission(&gate, alpha).await;
    gate.set_dialogue(alpha_terminal, false)
        .await
        .expect("disable");
    gate.set_dialogue(alpha_terminal, true)
        .await
        .expect("enable a fresh mailbox");
    let mail = CallEnvelope::tell(bravo, alpha, body("new lifetime"), now().plus(5_000));
    assert!(matches!(
        gate.command(
            bravo,
            Request::Send {
                envelope: mail.clone()
            }
        )
        .await,
        Answer::Accepted { .. }
    ));
    let requests = [
        Request::Send {
            envelope: CallEnvelope::tell(alpha, bravo, body("old send"), now().plus(5_000)),
        },
        Request::Ask {
            envelope: CallEnvelope::ask(alpha, bravo, body("old ask"), now().plus(5_000)),
        },
        Request::Receive {
            source: None,
            call: None,
            timeout_ms: 0,
        },
        Request::List { after: None },
    ];
    for request in requests {
        let mut pending = None;
        assert!(matches!(
            gate.command_owned(stale, request, &mut pending).await,
            Answer::Refused { .. }
        ));
        assert!(pending.is_none());
    }
    assert_eq!(gate.state.lock().await.courier.waiting(bravo), Some(0));
    let Answer::Received {
        envelope: Some(received),
    } = receive(&gate, alpha).await
    else {
        panic!("new mailbox retains its mail");
    };
    assert_eq!(received.message_id, mail.message_id);
}

#[tokio::test]
async fn a_disabled_hello_never_gains_wait_or_command_authority_from_later_enablement() {
    let gate = gate();
    let terminal = TerminalId::now();
    gate.launch(gate.mint(terminal).expect("mint"), || {
        Ok::<_, ()>(((), None))
    })
    .await
    .expect("register disabled");
    let session = session_id(terminal);
    let disabled = admission(&gate, session).await;
    assert!(gate.wait_slot(disabled).await.is_none());
    gate.set_dialogue(terminal, true).await.expect("enable");
    assert!(gate.wait_slot(disabled).await.is_none());
    let mut pending = None;
    assert!(matches!(
        gate.command_owned(disabled, Request::List { after: None }, &mut pending)
            .await,
        Answer::Refused { .. }
    ));
    assert!(pending.is_none());
    let current = admission(&gate, session).await;
    assert!(gate.wait_slot(current).await.is_some());
}

#[tokio::test]
async fn old_wait_permits_neither_block_nor_replenish_a_new_activations_allowance() {
    let Fleet {
        gate,
        alpha,
        alpha_terminal,
        ..
    } = fleet().await;
    let stale = admission(&gate, alpha).await;
    let mut old = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        old.push(gate.wait_slot(stale).await.expect("old allowance"));
    }
    gate.set_dialogue(alpha_terminal, false)
        .await
        .expect("disable");
    assert!(gate.wait_slot(stale).await.is_none());
    gate.set_dialogue(alpha_terminal, true)
        .await
        .expect("re-enable");
    let current = admission(&gate, alpha).await;
    let mut fresh = Vec::new();
    for _ in 0..SESSION_WAIT_SLOTS {
        fresh.push(
            gate.wait_slot(current)
                .await
                .expect("new allowance despite old held leases"),
        );
    }
    assert!(gate.wait_slot(current).await.is_none());
    drop(old);
    assert!(
        gate.wait_slot(current).await.is_none(),
        "old lease drops cannot mint new-generation permits"
    );
    drop(fresh);
    assert!(gate.wait_slot(current).await.is_some());
    assert!(gate.wait_slot(stale).await.is_none());
}

#[tokio::test]
async fn an_invalid_wait_cannot_acquire_cleanup_of_an_existing_call() {
    let Fleet {
        gate, alpha, bravo, ..
    } = fleet().await;
    let ask = CallEnvelope::ask(alpha, bravo, body("original"), now().plus(5_000));
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
            admission(&gate, alpha).await,
            Request::Receive {
                source: Some(bravo),
                call: Some(reference(&ask)),
                timeout_ms: Limits::INITIAL.max_deadline_millis + 1,
            },
            &mut pending,
        )
        .await;
    assert!(matches!(answer, Answer::Refused { .. }));
    assert!(
        pending.is_none(),
        "a rejected wait must not retire another invocation's ask"
    );
    assert_eq!(gate.state.lock().await.courier.active_calls(), 1);
}

#[tokio::test]
async fn list_contains_only_enabled_bound_mailboxes_and_ignores_disabled_cursors() {
    let gate = gate();
    let caller_terminal = TerminalId::now();
    let hidden_terminal = TerminalId::now();
    let later_terminal = TerminalId::now();
    for terminal in [caller_terminal, hidden_terminal, later_terminal] {
        gate.launch(gate.mint(terminal).expect("mint"), || {
            Ok::<_, ()>(((), Some(super::here())))
        })
        .await
        .expect("register root identity");
    }
    gate.set_dialogue(caller_terminal, true)
        .await
        .expect("caller enabled");
    gate.set_dialogue(later_terminal, true)
        .await
        .expect("later enabled");
    let caller = session_id(caller_terminal);
    let hidden = session_id(hidden_terminal);
    let later = session_id(later_terminal);
    let Answer::Sessions { sessions, next } =
        gate.command(caller, Request::List { after: None }).await
    else {
        panic!("listing");
    };
    assert_eq!(
        sessions.iter().map(|row| row.session).collect::<Vec<_>>(),
        vec![caller, later]
    );
    assert!(next.is_none());
    let Answer::Sessions { sessions, .. } = gate
        .command(
            caller,
            Request::List {
                after: Some(hidden),
            },
        )
        .await
    else {
        panic!("listing after disabled row");
    };
    assert_eq!(
        sessions.iter().map(|row| row.session).collect::<Vec<_>>(),
        vec![later]
    );
}
