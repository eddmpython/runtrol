use crate::wire::{Answer, Hello, Invocation, MAX_FRAME_BYTES, Request};
use crate::{
    BoundedUtf8, CallEnvelope, CallId, Courier, Limits, ManagedSessionId, MessageId, UnixMillis,
};

use super::{Fleet, NOW, accepted, body, fleet, later, snapshot};

#[test]
fn wire_commands_deliver_unicode_once_and_filter_without_consuming_other_mail() {
    let mut courier = Courier::new(Limits::INITIAL);
    let a = ManagedSessionId::now();
    let b = ManagedSessionId::now();
    let c = ManagedSessionId::now();
    for session in [a, b, c] {
        courier.session_started(session);
    }
    let now = UnixMillis(100);
    let body = |text: &str| BoundedUtf8::new(text.to_owned(), Limits::INITIAL.body_bytes).unwrap();
    let first = CallEnvelope::tell(c, b, body("unrelated"), UnixMillis(1000));
    courier.send(first.clone(), now).unwrap();
    let ask = CallEnvelope::ask(a, b, body("한국어 and English\n"), UnixMillis(1000));
    let invocation = Invocation {
        hello: Hello::new(a, "hidden".to_owned()),
        request: Some(Request::Send {
            envelope: ask.clone(),
        }),
    };
    let bytes = serde_json::to_vec(&invocation).unwrap();
    let decoded: Invocation = serde_json::from_slice(&bytes).unwrap();
    let Some(Request::Send { envelope }) = decoded.request else {
        panic!("send command");
    };
    courier.send(envelope, now).unwrap();
    assert!(courier.send(ask.clone(), now).is_err());
    let received = courier.receive_matching(b, Some(a), None, now).unwrap();
    assert_eq!(received.body, ask.body);
    assert_eq!(courier.waiting(b), Some(1));
    assert_eq!(courier.charged_bytes(), first.body.len());
    assert!(
        courier
            .answer(c, ask.message_id, MessageId::now(), body("wrong"), now)
            .is_err()
    );
    courier
        .answer(b, ask.message_id, MessageId::now(), body("정확한 답"), now)
        .unwrap();
    assert!(
        courier
            .answer(b, ask.message_id, MessageId::now(), body("duplicate"), now)
            .is_err()
    );
    assert!(
        courier
            .receive_matching(
                a,
                None,
                Some(crate::CallRef {
                    call_id: CallId::now(),
                    ask: MessageId::now()
                }),
                now
            )
            .is_none()
    );
    let reply = courier
        .receive_matching(
            a,
            Some(b),
            Some(crate::CallRef {
                call_id: ask.call_id,
                ask: ask.message_id,
            }),
            now,
        )
        .unwrap();
    assert_eq!(reply.reply_to, Some(ask.message_id));
    let output = serde_json::to_vec(&Answer::Received {
        envelope: Some(reply),
    })
    .unwrap();
    assert!(serde_json::from_slice::<Answer>(&output).is_ok());
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(
        courier.receive(b, now).unwrap().message_id,
        first.message_id
    );
    assert_eq!(courier.charged_bytes(), 0);
}

#[test]
fn maximum_escaped_body_fits_one_bounded_frame_without_leaking_into_debug() {
    let text = "\0".repeat(Limits::INITIAL.body_bytes);
    let envelope = CallEnvelope::tell(
        ManagedSessionId::now(),
        ManagedSessionId::now(),
        BoundedUtf8::new(text, Limits::INITIAL.body_bytes).unwrap(),
        UnixMillis(1000),
    );
    let invocation = Invocation {
        hello: Hello::new(envelope.source, "private-token".to_owned()),
        request: Some(Request::Send { envelope }),
    };
    let bytes = serde_json::to_vec(&invocation).unwrap();
    assert!(bytes.len() <= MAX_FRAME_BYTES);
    assert!(serde_json::from_slice::<Invocation>(&bytes).is_ok());
    assert!(!format!("{invocation:?}").contains("private-token"));
}

#[test]
fn abandoning_queued_or_received_call_releases_state_and_preserves_unrelated_mail() {
    for receive in [false, true] {
        let mut courier = Courier::new(Limits {
            mailbox_envelopes: 2,
            ..Limits::INITIAL
        });
        let a = ManagedSessionId::now();
        let b = ManagedSessionId::now();
        let c = ManagedSessionId::now();
        courier.session_started(a);
        courier.session_started(b);
        courier.session_started(c);
        let ask = CallEnvelope::ask(
            a,
            b,
            BoundedUtf8::new("body".into(), 16).unwrap(),
            UnixMillis(100),
        );
        courier.send(ask.clone(), UnixMillis(1)).unwrap();
        if receive {
            courier.receive(b, UnixMillis(2)).unwrap();
        }
        let other = CallEnvelope::tell(c, b, body("unrelated"), UnixMillis(200));
        courier.send(other.clone(), UnixMillis(2)).unwrap();
        courier.abandon_call(
            b,
            crate::CallRef {
                call_id: ask.call_id,
                ask: ask.message_id,
            },
        );
        assert_eq!(courier.active_calls(), 1);
        courier.abandon_call(
            a,
            crate::CallRef {
                call_id: ask.call_id,
                ask: ask.message_id,
            },
        );
        assert_eq!(courier.active_calls(), 0);
        assert_eq!(courier.charged_bytes(), other.body.len());
        assert_eq!(courier.next_deadline(), Some(other.deadline));
        assert_eq!(
            courier.receive(b, UnixMillis(2)).unwrap().message_id,
            other.message_id
        );
        assert_eq!(courier.charged_bytes(), 0);
        assert_eq!(courier.next_deadline(), None);
    }
}

#[test]
fn command_and_raw_replies_preserve_the_received_chain_and_its_hop_limit() {
    for command in [false, true] {
        let Fleet {
            mut courier,
            alpha,
            bravo,
            charlie,
        } = fleet(Limits {
            hop_count: 2,
            ..Limits::INITIAL
        });
        accepted(
            &mut courier,
            CallEnvelope::tell(alpha, bravo, body("start"), later(1_000)),
        );
        let first = courier.receive(bravo, NOW).expect("first hop");
        let ask = CallEnvelope::ask_onward(&first, charlie, body("question"), later(1_000));
        accepted(&mut courier, ask.clone());
        let heard = courier.receive(charlie, NOW).expect("second hop");
        assert_eq!(heard.hop_count, 2);
        assert_eq!(heard.visited.as_slice(), &[alpha, bravo]);
        assert_eq!(courier.charged_bytes(), 0);
        if command {
            courier
                .answer(
                    charlie,
                    ask.message_id,
                    MessageId::now(),
                    body("answer"),
                    NOW,
                )
                .expect("the command answers the exact ask");
        } else {
            let mut reply = CallEnvelope::reply(&heard, body("answer"), later(1_000));
            reply.hop_count = 0;
            reply.visited = crate::BoundedSessionSet::new();
            accepted(&mut courier, reply);
        }
        let reply = courier.receive(bravo, NOW).expect("the answer");
        assert_eq!(reply.hop_count, heard.hop_count);
        assert_eq!(reply.visited, heard.visited);
        assert_eq!(
            courier.send(
                CallEnvelope::forward(&reply, alpha, body("onward"), later(1_000)),
                NOW
            ),
            Err(crate::Refusal::HopBound {
                hops: 2,
                ceiling: 2,
            })
        );
        assert_eq!(courier.charged_bytes(), 0);
        assert_eq!(courier.active_calls(), 0);
    }
}

#[test]
fn command_and_raw_cancellation_preserve_the_chain_of_queued_and_received_asks() {
    for (command, received) in [(false, false), (false, true), (true, false), (true, true)] {
        let Fleet {
            mut courier,
            alpha,
            bravo,
            charlie,
        } = fleet(Limits::INITIAL);
        accepted(
            &mut courier,
            CallEnvelope::tell(alpha, bravo, body("start"), later(1_000)),
        );
        let first = courier.receive(bravo, NOW).expect("first hop");
        let ask = CallEnvelope::ask_onward(&first, charlie, body("question"), later(1_000));
        accepted(&mut courier, ask.clone());
        if received {
            courier.receive(charlie, NOW).expect("the ask");
        }
        if command {
            courier
                .cancel_call(bravo, ask.call_id, MessageId::now(), NOW)
                .expect("explicit cancellation");
        } else {
            let mut cancel = CallEnvelope::cancel(&ask, later(1_000));
            cancel.hop_count = 0;
            cancel.visited = crate::BoundedSessionSet::new();
            accepted(&mut courier, cancel);
        }
        let notice = courier.receive(charlie, NOW).expect("the cancellation");
        assert_eq!(notice.kind, crate::CallKind::Cancel);
        assert_eq!(notice.reply_to, Some(ask.message_id));
        assert_eq!(notice.hop_count, 2);
        assert_eq!(notice.visited.as_slice(), &[alpha, bravo]);
        assert_eq!(courier.receive(charlie, NOW), None);
        assert_eq!(courier.charged_bytes(), 0);
        assert_eq!(courier.active_calls(), 0);
    }
}

#[test]
fn an_ask_cannot_retain_a_visit_set_larger_than_the_couriers_own_limit() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        visited_sessions: 1,
        ..Limits::INITIAL
    });
    let mut ask = CallEnvelope::ask(alpha, bravo, body("question"), later(1_000));
    ask.visited = crate::BoundedSessionSet::new()
        .with(alpha, 2)
        .expect("first visit")
        .with(charlie, 2)
        .expect("the caller chose a larger limit");
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    assert_eq!(
        courier.send(ask, NOW),
        Err(crate::Refusal::VisitedBound(crate::VisitedBound {
            len: 2,
            ceiling: 1,
        }))
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
}

#[test]
fn a_full_mailbox_refuses_explicit_cancel_but_cannot_prevent_abandonment() {
    let Fleet {
        mut courier,
        alpha,
        bravo,
        charlie,
    } = fleet(Limits {
        mailbox_envelopes: 1,
        ..Limits::INITIAL
    });
    let ask = CallEnvelope::ask(alpha, bravo, body("question"), later(1_000));
    accepted(&mut courier, ask.clone());
    courier.receive(bravo, NOW).expect("the ask");
    let other = CallEnvelope::tell(charlie, bravo, body("unrelated"), later(2_000));
    accepted(&mut courier, other.clone());
    let before = snapshot(&courier, &[alpha, bravo, charlie]);
    let cancel_id = MessageId::now();
    assert_eq!(
        courier.cancel_call(alpha, ask.call_id, cancel_id, NOW),
        Err(crate::Refusal::MailboxEnvelopes {
            session: bravo,
            ceiling: 1,
        })
    );
    assert_eq!(snapshot(&courier, &[alpha, bravo, charlie]), before);
    assert_eq!(courier.state_of(cancel_id), None);
    courier.abandon_call(
        alpha,
        crate::CallRef {
            call_id: ask.call_id,
            ask: ask.message_id,
        },
    );
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), other.body.len());
    assert_eq!(
        courier.state_of(ask.message_id),
        Some(crate::DeliveryState::Cancelled)
    );
    assert_eq!(
        courier.receive(bravo, NOW).expect("other mail").message_id,
        other.message_id
    );
    assert_eq!(courier.receive(bravo, NOW), None);
}
