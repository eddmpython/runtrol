//! Call retirement must follow the exact ask, and expiry must win over a later cancellation.

use super::{Fleet, NOW, accepted, body, fleet, later, snapshot};
use crate::{CallEnvelope, CallRef, DeliveryState, Limits, Refusal};

#[test]
fn reading_a_departed_targets_reply_does_not_retire_a_new_call_with_the_same_id() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let old_ask = CallEnvelope::ask(alpha, bravo, body("old question"), later(500));
    accepted(&mut courier, old_ask.clone());
    let heard = courier.receive(bravo, NOW).expect("the old ask");
    let old_reply = CallEnvelope::reply(&heard, body("old answer"), later(500));
    accepted(&mut courier, old_reply.clone());
    assert_eq!(courier.session_ended(bravo).calls, 1);
    assert_eq!(courier.waiting(alpha), Some(1));

    let mut new_ask = CallEnvelope::ask(alpha, charlie, body("new question"), later(2_000));
    new_ask.call_id = old_ask.call_id;
    accepted(&mut courier, new_ask.clone());
    let heard = courier.receive(charlie, NOW).expect("the new ask");
    let new_reply = CallEnvelope::reply(&heard, body("new answer"), later(2_000));
    accepted(&mut courier, new_reply.clone());
    assert_eq!(courier.active_calls(), 1);

    let returned = courier.receive(alpha, NOW).expect("the retained old reply");
    assert_eq!(returned.message_id, old_reply.message_id);
    assert_eq!(courier.active_calls(), 1, "only the old ask was answered");
    assert_eq!(
        courier.state_of(new_ask.message_id),
        Some(DeliveryState::Replied)
    );
    assert_eq!(courier.charged_bytes(), new_reply.body.len());
    let returned = courier.receive(alpha, NOW).expect("the new reply");
    assert_eq!(returned.message_id, new_reply.message_id);
    assert_eq!(returned.reply_to, Some(new_ask.message_id));
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn expiring_a_departed_targets_reply_does_not_retire_a_new_call_with_the_same_id() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let old_ask = CallEnvelope::ask(alpha, bravo, body("old question"), later(500));
    accepted(&mut courier, old_ask.clone());
    let heard = courier.receive(bravo, NOW).expect("the old ask");
    let old_reply = CallEnvelope::reply(&heard, body("old answer"), later(500));
    accepted(&mut courier, old_reply.clone());
    courier.session_ended(bravo);

    let mut new_ask = CallEnvelope::ask(alpha, charlie, body("new question"), later(2_000));
    new_ask.call_id = old_ask.call_id;
    accepted(&mut courier, new_ask.clone());
    let swept = courier.sweep(later(500));
    assert_eq!(swept.messages, vec![old_reply.message_id]);
    assert!(
        swept.calls.is_empty(),
        "the retained reply owns no live call"
    );
    assert_eq!(swept.bytes, old_reply.body.len());
    assert_eq!(courier.active_calls(), 1);
    assert_eq!(courier.charged_bytes(), new_ask.body.len());
    assert_eq!(
        courier.state_of(old_ask.message_id),
        Some(DeliveryState::Replied)
    );
    assert_eq!(
        courier.state_of(old_reply.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(
        courier.state_of(new_ask.message_id),
        Some(DeliveryState::Accepted)
    );

    let heard = courier.receive(charlie, later(500)).expect("the new ask");
    let new_reply = CallEnvelope::reply(&heard, body("new answer"), later(2_000));
    courier
        .send(new_reply.clone(), later(500))
        .expect("the newer call still accepts its exact reply");
    let returned = courier.receive(alpha, later(500)).expect("the new reply");
    assert_eq!(returned.message_id, new_reply.message_id);
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn cancelling_an_expired_unread_ask_is_refused_without_changing_its_state() {
    assert_late_cancel_is_pure(false);
}

#[test]
fn cancelling_an_expired_received_ask_is_refused_without_changing_its_state() {
    assert_late_cancel_is_pure(true);
}

fn assert_late_cancel_is_pure(received: bool) {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), later(500));
    accepted(&mut courier, ask.clone());
    let expected_state = if received {
        courier.receive(bravo, NOW).expect("the ask is consumed");
        DeliveryState::Received
    } else {
        DeliveryState::Accepted
    };
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    let cancel = CallEnvelope::cancel(&ask, later(2_000));
    assert_eq!(
        courier.send(cancel.clone(), later(500)),
        Err(Refusal::CallExpired(ask.call_id))
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
    assert_eq!(courier.state_of(ask.message_id), Some(expected_state));
    assert_eq!(courier.state_of(cancel.message_id), None);

    let swept = courier.sweep(later(500));
    assert_eq!(swept.calls, vec![ask.call_id]);
    assert_eq!(swept.bytes, if received { 0 } else { ask.body.len() });
    assert_eq!(
        swept.messages,
        if received {
            vec![]
        } else {
            vec![ask.message_id]
        }
    );
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(courier.receive(bravo, later(500)), None);
}

#[test]
fn an_exact_wait_skips_a_retained_reply_to_an_older_ask_with_the_same_call_id() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let old_ask = CallEnvelope::ask(alpha, bravo, body("old question"), later(1_000));
    accepted(&mut courier, old_ask.clone());
    let heard = courier.receive(bravo, NOW).expect("the old ask");
    let old_reply = CallEnvelope::reply(&heard, body("old answer"), later(1_000));
    accepted(&mut courier, old_reply.clone());
    courier.session_ended(bravo);
    let old_ref = CallRef {
        call_id: old_ask.call_id,
        ask: old_ask.message_id,
    };

    let mut new_ask = CallEnvelope::ask(alpha, charlie, body("new question"), later(2_000));
    new_ask.call_id = old_ask.call_id;
    accepted(&mut courier, new_ask.clone());
    let new_ref = CallRef {
        call_id: new_ask.call_id,
        ask: new_ask.message_id,
    };
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    assert!(!courier.owns_call(alpha, old_ref));
    assert!(courier.owns_call(alpha, new_ref));
    assert_eq!(
        courier.receive_matching(alpha, None, Some(new_ref), NOW),
        None
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);

    let heard = courier.receive(charlie, NOW).expect("the new ask");
    let new_reply = CallEnvelope::reply(&heard, body("new answer"), later(2_000));
    accepted(&mut courier, new_reply.clone());
    let received = courier
        .receive_matching(alpha, None, Some(new_ref), NOW)
        .expect("the exact new reply");
    assert_eq!(received.message_id, new_reply.message_id);
    assert_eq!(courier.charged_bytes(), old_reply.body.len());
    assert_eq!(courier.active_calls(), 0);
    let received = courier
        .receive_matching(alpha, Some(bravo), Some(old_ref), NOW)
        .expect("the departed target's retained reply");
    assert_eq!(received.message_id, old_reply.message_id);
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn abandoning_an_old_wait_never_cancels_a_new_ask_with_the_same_call_id() {
    for received in [false, true] {
        let Fleet {
            mut courier,
            alpha,
            bravo,
            charlie,
        } = fleet(Limits::INITIAL);
        let old_ask = CallEnvelope::ask(alpha, bravo, body("old question"), later(1_000));
        accepted(&mut courier, old_ask.clone());
        let old_ref = CallRef {
            call_id: old_ask.call_id,
            ask: old_ask.message_id,
        };
        courier.abandon_call(alpha, old_ref);

        let mut new_ask = CallEnvelope::ask(alpha, charlie, body("new question"), later(2_000));
        new_ask.call_id = old_ask.call_id;
        accepted(&mut courier, new_ask.clone());
        if received {
            courier.receive(charlie, NOW).expect("the new ask");
        }
        let new_ref = CallRef {
            call_id: new_ask.call_id,
            ask: new_ask.message_id,
        };
        let before = snapshot(&courier, &[alpha, bravo, charlie]);
        courier.abandon_call(alpha, old_ref);
        assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
        assert!(courier.owns_call(alpha, new_ref));
        assert_eq!(
            courier.state_of(new_ask.message_id),
            Some(if received {
                DeliveryState::Received
            } else {
                DeliveryState::Accepted
            })
        );
        courier.abandon_call(alpha, new_ref);
        assert_eq!(courier.active_calls(), 0);
        assert_eq!(courier.charged_bytes(), 0);
        assert_eq!(
            courier.state_of(new_ask.message_id),
            Some(DeliveryState::Cancelled)
        );
    }
}

#[test]
fn abandoning_a_completed_call_keeps_its_exact_reply_readable() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), later(1_000));
    accepted(&mut courier, ask.clone());
    let heard = courier.receive(bravo, NOW).expect("the ask");
    let reply = CallEnvelope::reply(&heard, body("answer"), later(1_000));
    accepted(&mut courier, reply.clone());
    let call = CallRef {
        call_id: ask.call_id,
        ask: ask.message_id,
    };
    let before = snapshot(&courier, &[alpha, bravo]);
    courier.abandon_call(alpha, call);
    assert_eq!(snapshot(&courier, &[alpha, bravo]), before);
    let received = courier
        .receive_matching(alpha, None, Some(call), NOW)
        .expect("the completed reply");
    assert_eq!(received.message_id, reply.message_id);
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
}
