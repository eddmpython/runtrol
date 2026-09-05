//! Mutation tests: every ceiling and every rule is reached by an envelope that breaks it, and a refused
//! envelope leaves the courier exactly as it was.

mod commands;
mod lifetime;
mod reserved;

use super::*;
use crate::body::BoundedUtf8;
use crate::envelope::VisitedBound;
use crate::id::RoomId;

const NOW: UnixMillis = UnixMillis(1_800_000_000_000);

fn later(millis: u64) -> UnixMillis {
    NOW.plus(millis)
}

fn body(text: &str) -> BoundedUtf8 {
    BoundedUtf8::new(text.to_owned(), Limits::INITIAL.body_bytes).expect("the body fits")
}

struct Fleet {
    courier: Courier,
    alpha: ManagedSessionId,
    bravo: ManagedSessionId,
    charlie: ManagedSessionId,
}

fn fleet(limits: Limits) -> Fleet {
    let mut courier = Courier::new(limits);
    let alpha = ManagedSessionId::now();
    let bravo = ManagedSessionId::now();
    let charlie = ManagedSessionId::now();
    for session in [alpha, bravo, charlie] {
        assert!(courier.session_started(session));
    }
    Fleet {
        courier,
        alpha,
        bravo,
        charlie,
    }
}

fn accepted(courier: &mut Courier, envelope: CallEnvelope) -> Receipt {
    courier
        .send(envelope, NOW)
        .expect("the envelope is admitted")
}

/// Everything a refusal must leave alone.
fn snapshot(
    courier: &Courier,
    sessions: &[ManagedSessionId],
) -> (usize, usize, Vec<Option<usize>>) {
    (
        courier.charged_bytes(),
        courier.active_calls(),
        sessions
            .iter()
            .map(|session| courier.waiting(*session))
            .collect(),
    )
}

#[test]
fn a_tell_is_accepted_then_received_once_and_charged_only_while_it_waits() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let tell = CallEnvelope::tell(alpha, bravo, body("hello"), later(1_000));
    let receipt = accepted(&mut courier, tell.clone());
    assert_eq!(
        receipt,
        Receipt {
            message_id: tell.message_id,
            call_id: tell.call_id,
            state: DeliveryState::Accepted,
        }
    );
    assert_eq!(
        courier.state_of(tell.message_id),
        Some(DeliveryState::Accepted)
    );
    assert_eq!(courier.charged_bytes(), 5);
    assert_eq!(courier.waiting(bravo), Some(1));
    assert_eq!(courier.waiting(alpha), Some(0));
    assert_eq!(courier.active_calls(), 0, "a tell opens no call");

    let heard = courier
        .receive(bravo, NOW)
        .expect("the target is handed the envelope");
    assert_eq!(heard.message_id, tell.message_id);
    assert_eq!(heard.body.as_str(), "hello");
    assert_eq!(heard.hop_count, 1, "delivery is one hop");
    assert_eq!(heard.visited.as_slice(), &[alpha]);
    assert_eq!(
        courier.state_of(tell.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(
        courier.receive(bravo, NOW),
        None,
        "an envelope is handed over once"
    );
    assert_eq!(courier.receive(alpha, NOW), None);
}

#[test]
fn an_envelope_of_another_version_is_refused_untouched() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    let mut foreign = CallEnvelope::tell(alpha, bravo, body("x"), later(1_000));
    foreign.protocol_version = PROTOCOL_VERSION.saturating_add(1);
    assert_eq!(
        courier.send(foreign.clone(), NOW),
        Err(Refusal::UnsupportedVersion { offered: 2 })
    );
    assert_eq!(
        courier.state_of(foreign.message_id),
        None,
        "a refused message is not remembered"
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
}

#[test]
fn a_session_cannot_send_to_itself_and_only_live_sessions_send_or_receive() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, alpha, body("me"), later(1_000)),
            NOW
        ),
        Err(Refusal::SelfSend)
    );
    let stranger = ManagedSessionId::now();
    assert_eq!(
        courier.send(
            CallEnvelope::tell(stranger, bravo, body("x"), later(1_000)),
            NOW
        ),
        Err(Refusal::UnknownSource(stranger))
    );
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, stranger, body("x"), later(1_000)),
            NOW
        ),
        Err(Refusal::UnknownTarget(stranger))
    );
    assert!(!courier.is_live(stranger));
    assert_eq!(courier.waiting(stranger), None);
    assert_eq!(courier.receive(stranger, NOW), None);
    assert!(
        !courier.session_started(alpha),
        "a live session is not started twice"
    );
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn a_room_envelope_is_refused_while_rooms_are_closed() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let mut envelope = CallEnvelope::tell(alpha, bravo, body("x"), later(1_000));
    envelope.room_id = Some(RoomId::now());
    assert_eq!(courier.send(envelope, NOW), Err(Refusal::RoomsClosed));
    assert_eq!(courier.waiting(bravo), Some(0));
}

#[test]
fn the_same_message_identifier_is_refused_again_before_and_after_it_was_read() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let tell = CallEnvelope::tell(alpha, bravo, body("once"), later(1_000));
    accepted(&mut courier, tell.clone());
    assert_eq!(
        courier.send(tell.clone(), NOW),
        Err(Refusal::DuplicateMessage(tell.message_id))
    );
    let mut reused = CallEnvelope::tell(bravo, alpha, body("other"), later(1_000));
    reused.message_id = tell.message_id;
    assert_eq!(
        courier.send(reused, NOW),
        Err(Refusal::DuplicateMessage(tell.message_id)),
        "a different envelope under the same identifier is the same duplicate"
    );
    courier.receive(bravo, NOW).expect("read once");
    assert_eq!(
        courier.send(tell.clone(), NOW),
        Err(Refusal::DuplicateMessage(tell.message_id)),
        "reading it does not make room for a second copy"
    );
    assert_eq!(courier.waiting(bravo), Some(0));
    assert_eq!(courier.waiting(alpha), Some(0));
}

#[test]
fn a_deadline_at_or_before_now_is_refused_and_so_is_one_past_the_ceiling() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    assert_eq!(
        courier.send(CallEnvelope::tell(alpha, bravo, body("x"), NOW), NOW),
        Err(Refusal::DeadlinePassed {
            deadline: NOW,
            now: NOW
        })
    );
    let earlier = UnixMillis(NOW.0.saturating_sub(1));
    assert_eq!(
        courier.send(CallEnvelope::tell(alpha, bravo, body("x"), earlier), NOW),
        Err(Refusal::DeadlinePassed {
            deadline: earlier,
            now: NOW
        })
    );
    let ceiling = Limits::INITIAL.max_deadline_millis;
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, bravo, body("x"), later(ceiling.saturating_add(1))),
            NOW
        ),
        Err(Refusal::DeadlineTooFar {
            millis: 600_001,
            ceiling: 600_000
        })
    );
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, body("x"), later(ceiling)),
    );
    accepted(
        &mut courier,
        CallEnvelope::tell(
            bravo,
            alpha,
            body("y"),
            Limits::INITIAL.default_deadline(NOW),
        ),
    );
    assert_eq!(courier.waiting(bravo), Some(1));
    assert_eq!(courier.waiting(alpha), Some(1));
}

#[test]
fn mail_that_waits_past_its_deadline_expires_unread_and_releases_its_bytes() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let tell = CallEnvelope::tell(alpha, bravo, body("late"), later(100));
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), later(200));
    accepted(&mut courier, tell.clone());
    accepted(&mut courier, ask.clone());
    assert_eq!(courier.active_calls(), 1);
    assert_eq!(courier.charged_bytes(), 12);

    assert_eq!(courier.sweep(later(99)), Swept::default());
    assert_eq!(
        courier.sweep(later(100)),
        Swept {
            messages: vec![tell.message_id],
            calls: vec![],
            bytes: 4,
        }
    );
    assert_eq!(
        courier.state_of(tell.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(courier.charged_bytes(), 8);

    assert_eq!(
        courier.receive(bravo, later(200)),
        None,
        "an ask that expired while waiting is not handed over"
    );
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(courier.waiting(bravo), Some(0));
}

#[test]
fn a_received_ask_whose_reply_never_comes_expires_at_the_call_deadline() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(300));
    accepted(&mut courier, ask.clone());
    let heard = courier.receive(bravo, NOW).expect("the ask is handed over");
    assert_eq!(courier.sweep(later(299)), Swept::default());
    assert_eq!(
        courier.sweep(later(300)),
        Swept {
            messages: vec![],
            calls: vec![ask.call_id],
            bytes: 0,
        }
    );
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(
        courier.send(
            CallEnvelope::reply(&heard, body("too late"), later(400)),
            later(301)
        ),
        Err(Refusal::NoSuchCall(ask.call_id))
    );
}

#[test]
fn hops_are_counted_on_delivery_and_the_hop_past_the_ceiling_is_refused() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        hop_count: 2,
        ..Limits::INITIAL
    });
    let delta = ManagedSessionId::now();
    assert!(courier.session_started(delta));

    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, body("1"), later(1_000)),
    );
    let at_bravo = courier.receive(bravo, NOW).expect("one hop");
    assert_eq!(at_bravo.hop_count, 1);
    accepted(
        &mut courier,
        CallEnvelope::forward(&at_bravo, charlie, body("2"), later(1_000)),
    );
    let at_charlie = courier.receive(charlie, NOW).expect("two hops");
    assert_eq!(at_charlie.hop_count, 2);
    assert_eq!(at_charlie.visited.as_slice(), &[alpha, bravo]);

    assert_eq!(
        courier.send(
            CallEnvelope::forward(&at_charlie, delta, body("3"), later(1_000)),
            NOW
        ),
        Err(Refusal::HopBound {
            hops: 2,
            ceiling: 2
        })
    );
    assert_eq!(
        courier.send(
            CallEnvelope::ask_onward(&at_charlie, delta, body("3"), later(1_000)),
            NOW
        ),
        Err(Refusal::HopBound {
            hops: 2,
            ceiling: 2
        })
    );
    assert_eq!(courier.waiting(delta), Some(0));
    accepted(
        &mut courier,
        CallEnvelope::tell(charlie, delta, body("fresh"), later(1_000)),
    );
}

#[test]
fn a_forward_back_to_a_visited_session_is_a_cycle_but_a_fresh_message_is_not() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, body("1"), later(1_000)),
    );
    let at_bravo = courier.receive(bravo, NOW).expect("delivered");
    assert_eq!(
        courier.send(
            CallEnvelope::forward(&at_bravo, alpha, body("back"), later(1_000)),
            NOW
        ),
        Err(Refusal::Cycle(alpha))
    );
    accepted(
        &mut courier,
        CallEnvelope::tell(bravo, alpha, body("fresh"), later(1_000)),
    );
    accepted(
        &mut courier,
        CallEnvelope::forward(&at_bravo, charlie, body("on"), later(1_000)),
    );
    let at_charlie = courier.receive(charlie, NOW).expect("delivered");
    assert_eq!(
        courier.send(
            CallEnvelope::forward(&at_charlie, alpha, body("x"), later(1_000)),
            NOW
        ),
        Err(Refusal::Cycle(alpha))
    );
    assert_eq!(
        courier.send(
            CallEnvelope::forward(&at_charlie, bravo, body("x"), later(1_000)),
            NOW
        ),
        Err(Refusal::Cycle(bravo))
    );
    assert_eq!(courier.waiting(alpha), Some(1));
    assert_eq!(courier.waiting(bravo), Some(0));
}

#[test]
fn a_chain_that_visited_as_many_sessions_as_the_ceiling_is_refused_before_routing() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        visited_sessions: 2,
        hop_count: 10,
        ..Limits::INITIAL
    });
    let delta = ManagedSessionId::now();
    assert!(courier.session_started(delta));
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, body("1"), later(1_000)),
    );
    let at_bravo = courier.receive(bravo, NOW).expect("delivered");
    accepted(
        &mut courier,
        CallEnvelope::forward(&at_bravo, charlie, body("2"), later(1_000)),
    );
    let at_charlie = courier.receive(charlie, NOW).expect("delivered");
    assert_eq!(at_charlie.visited.len(), 2);
    assert_eq!(
        courier.send(
            CallEnvelope::forward(&at_charlie, delta, body("3"), later(1_000)),
            NOW
        ),
        Err(Refusal::VisitedBound(VisitedBound { len: 2, ceiling: 2 }))
    );
    assert_eq!(courier.waiting(delta), Some(0));
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn a_mailbox_holds_its_envelope_ceiling_and_never_drops_old_mail() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        mailbox_envelopes: 2,
        ..Limits::INITIAL
    });
    let first = CallEnvelope::tell(alpha, bravo, body("first"), later(1_000));
    let second = CallEnvelope::tell(alpha, bravo, body("second"), later(1_000));
    accepted(&mut courier, first.clone());
    accepted(&mut courier, second.clone());
    let third = CallEnvelope::tell(alpha, bravo, body("third"), later(1_000));
    assert_eq!(
        courier.send(third.clone(), NOW),
        Err(Refusal::MailboxEnvelopes {
            session: bravo,
            ceiling: 2
        })
    );
    assert_eq!(courier.state_of(third.message_id), None);
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, charlie, body("elsewhere"), later(1_000)),
    );
    assert_eq!(
        courier
            .receive(bravo, NOW)
            .map(|envelope| envelope.message_id),
        Some(first.message_id),
        "the oldest mail is still first"
    );
    assert_eq!(
        courier
            .receive(bravo, NOW)
            .map(|envelope| envelope.message_id),
        Some(second.message_id)
    );
    accepted(&mut courier, third);
}

#[test]
fn the_byte_ceilings_count_bodies_per_mailbox_and_across_the_runtime() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        body_bytes: 64,
        mailbox_bytes: 100,
        runtime_bytes: 150,
        ..Limits::INITIAL
    });
    let sixty = BoundedUtf8::new("6".repeat(60), 64).expect("fits");
    let fifty = BoundedUtf8::new("5".repeat(50), 64).expect("fits");
    let forty = BoundedUtf8::new("4".repeat(40), 64).expect("fits");
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, sixty.clone(), later(1_000)),
    );
    assert_eq!(
        courier.send(CallEnvelope::tell(alpha, bravo, fifty, later(1_000)), NOW),
        Err(Refusal::MailboxBytes {
            session: bravo,
            ceiling: 100
        })
    );
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, charlie, sixty, later(1_000)),
    );
    assert_eq!(courier.charged_bytes(), 120);
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, charlie, forty.clone(), later(1_000)),
            NOW
        ),
        Err(Refusal::RuntimeBytes { ceiling: 150 }),
        "the mailbox has room but the Runtime does not"
    );
    courier.receive(bravo, NOW).expect("read sixty bytes");
    assert_eq!(courier.charged_bytes(), 60);
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, charlie, forty, later(1_000)),
    );
    assert_eq!(courier.charged_bytes(), 100);
}

#[test]
fn an_ask_past_the_active_call_ceiling_is_refused_while_a_tell_still_passes() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        active_calls: 2,
        ..Limits::INITIAL
    });
    accepted(
        &mut courier,
        CallEnvelope::ask(alpha, bravo, body("1"), later(1_000)),
    );
    let second = CallEnvelope::ask(alpha, bravo, body("2"), later(1_000));
    accepted(&mut courier, second);
    assert_eq!(
        courier.send(
            CallEnvelope::ask(alpha, charlie, body("3"), later(1_000)),
            NOW
        ),
        Err(Refusal::TooManyCalls { ceiling: 2 })
    );
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, charlie, body("3"), later(1_000)),
    );
    let heard = courier.receive(bravo, NOW).expect("the first ask");
    accepted(
        &mut courier,
        CallEnvelope::reply(&heard, body("r"), later(1_000)),
    );
    courier.receive(alpha, NOW).expect("the reply");
    assert_eq!(courier.active_calls(), 1);
    accepted(
        &mut courier,
        CallEnvelope::ask(alpha, charlie, body("4"), later(1_000)),
    );
}

#[test]
fn an_ask_is_answered_by_exactly_one_correlated_reply() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), later(1_000));
    accepted(&mut courier, ask.clone());
    assert_eq!(courier.active_calls(), 1);
    let heard = courier.receive(bravo, NOW).expect("the ask is handed over");
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(courier.charged_bytes(), 0, "a read body is released");

    let reply = CallEnvelope::reply(&heard, body("answer"), later(1_000));
    accepted(&mut courier, reply.clone());
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Replied)
    );
    assert_eq!(
        courier.state_of(reply.message_id),
        Some(DeliveryState::Accepted)
    );
    assert_eq!(courier.waiting(alpha), Some(1));
    assert_eq!(courier.charged_bytes(), 6);
    assert_eq!(
        courier.send(
            CallEnvelope::reply(&heard, body("again"), later(1_000)),
            NOW
        ),
        Err(Refusal::AlreadyReplied(ask.call_id))
    );

    let returned = courier
        .receive(alpha, NOW)
        .expect("the reply is handed over");
    assert_eq!(returned.kind, CallKind::Reply);
    assert_eq!(returned.call_id, ask.call_id);
    assert_eq!(returned.reply_to, Some(ask.message_id));
    assert_eq!(returned.body.as_str(), "answer");
    assert_eq!(
        courier.state_of(reply.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Replied)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(
        courier.send(CallEnvelope::reply(&heard, body("late"), later(1_000)), NOW),
        Err(Refusal::NoSuchCall(ask.call_id))
    );
}

#[test]
fn a_reply_is_accepted_only_from_the_target_to_the_source_for_the_asks_own_message() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, ask.clone());
    let heard = courier.receive(bravo, NOW).expect("delivered");
    let before = snapshot(&courier, &[alpha, bravo, charlie]);

    let mut from_charlie = CallEnvelope::reply(&heard, body("r"), later(1_000));
    from_charlie.source = charlie;
    assert_eq!(
        courier.send(from_charlie, NOW),
        Err(Refusal::WrongSource { expected: bravo })
    );
    let mut to_charlie = CallEnvelope::reply(&heard, body("r"), later(1_000));
    to_charlie.target = charlie;
    assert_eq!(
        courier.send(to_charlie, NOW),
        Err(Refusal::WrongTarget { expected: alpha })
    );
    let mut wrong_message = CallEnvelope::reply(&heard, body("r"), later(1_000));
    let offered = MessageId::now();
    wrong_message.reply_to = Some(offered);
    assert_eq!(
        courier.send(wrong_message, NOW),
        Err(Refusal::WrongMessage {
            expected: ask.message_id,
            offered
        })
    );
    let mut no_call = CallEnvelope::reply(&heard, body("r"), later(1_000));
    no_call.call_id = CallId::now();
    assert_eq!(
        courier.send(no_call.clone(), NOW),
        Err(Refusal::NoSuchCall(no_call.call_id))
    );
    let mut bare = CallEnvelope::reply(&heard, body("r"), later(1_000));
    bare.reply_to = None;
    assert_eq!(
        courier.send(bare, NOW),
        Err(Refusal::MissingReplyTo(CallKind::Reply))
    );
    let mut odd = CallEnvelope::tell(alpha, bravo, body("t"), later(1_000));
    odd.reply_to = Some(ask.message_id);
    assert_eq!(
        courier.send(odd, NOW),
        Err(Refusal::UnexpectedReplyTo(CallKind::Tell))
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
    accepted(
        &mut courier,
        CallEnvelope::reply(&heard, body("r"), later(1_000)),
    );
}

#[test]
fn a_reply_before_the_target_read_the_ask_is_refused() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, ask.clone());
    assert_eq!(
        courier.send(CallEnvelope::reply(&ask, body("r"), later(1_000)), NOW),
        Err(Refusal::ReplyBeforeReceipt(ask.message_id))
    );
    let heard = courier.receive(bravo, NOW).expect("delivered");
    accepted(
        &mut courier,
        CallEnvelope::reply(&heard, body("r"), later(1_000)),
    );
}

#[test]
fn a_reply_after_the_call_deadline_is_refused_and_a_reply_cannot_outlive_its_call() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(500));
    accepted(&mut courier, ask.clone());
    let heard = courier.receive(bravo, NOW).expect("delivered");
    assert_eq!(
        courier.send(
            CallEnvelope::reply(&heard, body("late"), later(900)),
            later(500)
        ),
        Err(Refusal::CallExpired(ask.call_id))
    );
    // The refusal changed nothing: the call is still open and its ask still received. The sweep, not the
    // refused reply, is what expires it.
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(courier.active_calls(), 1);
    assert_eq!(
        courier.sweep(later(500)),
        Swept {
            messages: vec![],
            calls: vec![ask.call_id],
            bytes: 0,
        }
    );
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Expired)
    );
    assert_eq!(courier.active_calls(), 0);

    let again = CallEnvelope::ask(alpha, bravo, body("q"), later(500));
    accepted(&mut courier, again.clone());
    let heard = courier.receive(bravo, NOW).expect("delivered");
    let reply = CallEnvelope::reply(&heard, body("r"), later(5_000));
    assert_eq!(
        courier
            .send(reply.clone(), later(100))
            .map(|receipt| receipt.state),
        Ok(DeliveryState::Accepted)
    );
    assert_eq!(courier.sweep(later(499)), Swept::default());
    assert_eq!(
        courier.sweep(later(500)),
        Swept {
            messages: vec![reply.message_id],
            calls: vec![again.call_id],
            bytes: 1,
        },
        "the reply expires with the call it answers"
    );
    assert_eq!(
        courier.state_of(again.message_id),
        Some(DeliveryState::Replied)
    );
    assert_eq!(courier.receive(alpha, later(500)), None);
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn a_cancel_withdraws_an_unread_ask_ends_the_call_and_still_reaches_the_target() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        ..
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, ask.clone());
    assert_eq!(courier.charged_bytes(), 1);
    let cancel = CallEnvelope::cancel(&ask, later(1_000));
    accepted(&mut courier, cancel.clone());
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(DeliveryState::Cancelled)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0, "the withdrawn body is released");
    assert_eq!(courier.waiting(bravo), Some(1), "only the cancel waits");
    let heard = courier
        .receive(bravo, NOW)
        .expect("the cancel is handed over");
    assert_eq!(heard.kind, CallKind::Cancel);
    assert_eq!(heard.reply_to, Some(ask.message_id));
    assert_eq!(
        courier.state_of(cancel.message_id),
        Some(DeliveryState::Received)
    );

    let second = CallEnvelope::ask(alpha, bravo, body("q2"), later(1_000));
    accepted(&mut courier, second.clone());
    let heard = courier.receive(bravo, NOW).expect("delivered");
    accepted(&mut courier, CallEnvelope::cancel(&second, later(1_000)));
    assert_eq!(
        courier.state_of(second.message_id),
        Some(DeliveryState::Cancelled)
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(
        courier.receive(bravo, NOW).map(|envelope| envelope.kind),
        Some(CallKind::Cancel)
    );
    assert_eq!(
        courier.send(CallEnvelope::reply(&heard, body("r"), later(1_000)), NOW),
        Err(Refusal::NoSuchCall(second.call_id))
    );
}

#[test]
fn a_cancel_from_anyone_but_the_asker_or_after_the_reply_is_refused() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, ask.clone());
    let mut from_bravo = CallEnvelope::cancel(&ask, later(1_000));
    from_bravo.source = bravo;
    from_bravo.target = alpha;
    assert_eq!(
        courier.send(from_bravo, NOW),
        Err(Refusal::WrongSource { expected: alpha })
    );
    let mut to_charlie = CallEnvelope::cancel(&ask, later(1_000));
    to_charlie.target = charlie;
    assert_eq!(
        courier.send(to_charlie, NOW),
        Err(Refusal::WrongTarget { expected: bravo })
    );
    let heard = courier.receive(bravo, NOW).expect("delivered");
    accepted(
        &mut courier,
        CallEnvelope::reply(&heard, body("r"), later(1_000)),
    );
    assert_eq!(
        courier.send(CallEnvelope::cancel(&ask, later(1_000)), NOW),
        Err(Refusal::AlreadyReplied(ask.call_id))
    );
    assert_eq!(courier.waiting(alpha), Some(1));
}

#[test]
fn a_session_ending_releases_its_mail_and_its_calls_immediately() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits::INITIAL);
    let unread = CallEnvelope::tell(alpha, bravo, body("one"), later(1_000));
    accepted(&mut courier, unread.clone());
    let queued_elsewhere = CallEnvelope::ask(bravo, charlie, body("two"), later(1_000));
    accepted(&mut courier, queued_elsewhere.clone());
    let owed = CallEnvelope::ask(charlie, bravo, body("hey"), later(1_000));
    accepted(&mut courier, owed.clone());
    let answered = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, answered.clone());
    let first_read = courier.receive(bravo, NOW).expect("the first unread");
    assert_eq!(first_read.message_id, unread.message_id);
    let heard = courier.receive(bravo, NOW).expect("the ask from charlie");
    assert_eq!(heard.message_id, owed.message_id);
    let heard = courier.receive(bravo, NOW).expect("the ask from alpha");
    assert_eq!(heard.message_id, answered.message_id);
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, body("one"), later(1_000)),
    );
    let reply = CallEnvelope::reply(&heard, body("r"), later(1_000));
    accepted(&mut courier, reply.clone());
    assert_eq!(courier.active_calls(), 3);
    assert_eq!(courier.charged_bytes(), 7);

    assert_eq!(
        courier.session_ended(bravo),
        Released {
            envelopes: 2,
            calls: 3,
            bytes: 6,
        }
    );
    assert!(!courier.is_live(bravo));
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(
        courier.charged_bytes(),
        1,
        "only the reply on its way remains"
    );
    assert_eq!(courier.waiting(charlie), Some(0));
    assert_eq!(
        courier.state_of(queued_elsewhere.message_id),
        Some(DeliveryState::Cancelled)
    );
    assert_eq!(
        courier.state_of(owed.message_id),
        Some(DeliveryState::Cancelled)
    );
    assert_eq!(
        courier.state_of(answered.message_id),
        Some(DeliveryState::Replied)
    );
    let returned = courier
        .receive(alpha, NOW)
        .expect("the reply still arrives");
    assert_eq!(returned.message_id, reply.message_id);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(courier.session_ended(bravo), Released::default());
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, bravo, body("x"), later(1_000)),
            NOW
        ),
        Err(Refusal::UnknownTarget(bravo))
    );
}

#[test]
fn a_fresh_message_cannot_reuse_an_open_call() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        active_calls: 4,
        ..Limits::INITIAL
    });
    let ask = CallEnvelope::ask(alpha, bravo, body("q"), later(1_000));
    accepted(&mut courier, ask.clone());
    let before = snapshot(&courier, &[alpha, bravo, charlie]);

    // A second ask under the same call would overwrite the first's record and hide it from the ceiling.
    let mut reused = CallEnvelope::ask(alpha, charlie, body("2"), later(1_000));
    reused.call_id = ask.call_id;
    assert_eq!(
        courier.send(reused, NOW),
        Err(Refusal::CallInUse(ask.call_id))
    );
    // A tell under the same call is refused too: a tell groups no request and its reply.
    let mut tell = CallEnvelope::tell(alpha, charlie, body("t"), later(1_000));
    tell.call_id = ask.call_id;
    assert_eq!(
        courier.send(tell, NOW),
        Err(Refusal::CallInUse(ask.call_id))
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
    assert_eq!(courier.active_calls(), 1);

    // The original ask still resolves and retires cleanly: nothing was overwritten.
    let heard = courier
        .receive(bravo, NOW)
        .expect("the one ask is delivered");
    assert_eq!(heard.message_id, ask.message_id);
    accepted(
        &mut courier,
        CallEnvelope::reply(&heard, body("r"), later(1_000)),
    );
    courier.receive(alpha, NOW).expect("the reply");
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), 0);
    // The call is free again once it resolved.
    let fresh = CallEnvelope::ask(alpha, bravo, body("again"), later(1_000));
    accepted(&mut courier, fresh);
}

#[test]
fn the_courier_enforces_its_own_body_ceiling_however_the_body_was_built() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        body_bytes: 16,
        ..Limits::INITIAL
    });
    // A body built with a larger ceiling than the courier's is still refused by the courier.
    let oversized =
        BoundedUtf8::new("x".repeat(17), usize::MAX).expect("built with a wide ceiling");
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    assert_eq!(
        courier.send(
            CallEnvelope::tell(alpha, bravo, oversized, later(1_000)),
            NOW
        ),
        Err(Refusal::BodyTooLarge {
            len: 17,
            ceiling: 16
        })
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
    // A body at the ceiling passes.
    let at_ceiling = BoundedUtf8::new("y".repeat(16), usize::MAX).expect("built");
    accepted(
        &mut courier,
        CallEnvelope::tell(alpha, bravo, at_ceiling, later(1_000)),
    );
    assert_eq!(courier.charged_bytes(), 16);
}

#[test]
fn retired_identifiers_are_remembered_up_to_the_ceiling_and_the_oldest_is_forgotten_first() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        remembered_messages: 2,
        ..Limits::INITIAL
    });
    let pending = CallEnvelope::ask(alpha, charlie, body("wait"), later(1_000));
    accepted(&mut courier, pending.clone());
    let first = CallEnvelope::tell(alpha, bravo, body("1"), later(1_000));
    let second = CallEnvelope::tell(alpha, bravo, body("2"), later(1_000));
    let third = CallEnvelope::tell(alpha, bravo, body("3"), later(1_000));
    for tell in [&first, &second, &third] {
        accepted(&mut courier, tell.clone());
        courier.receive(bravo, NOW).expect("read");
    }
    assert_eq!(courier.state_of(first.message_id), None);
    assert_eq!(
        courier.state_of(second.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(
        courier.state_of(third.message_id),
        Some(DeliveryState::Received)
    );
    assert_eq!(
        courier.send(second.clone(), NOW),
        Err(Refusal::DuplicateMessage(second.message_id)),
        "a remembered identifier is still a duplicate"
    );
    accepted(&mut courier, first);
    assert_eq!(
        courier.state_of(pending.message_id),
        Some(DeliveryState::Accepted),
        "a live identifier is never forgotten, however many retire after it"
    );
}
