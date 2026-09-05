//! Mutation evidence for room authority, round bounds, and exact body retirement.

use super::*;
use crate::{CallKind, Limits};

const NOW: UnixMillis = UnixMillis(100);
const DEADLINE: UnixMillis = UnixMillis(1_000);

fn body(text: &str) -> BoundedUtf8 {
    BoundedUtf8::new(text.to_owned(), Limits::INITIAL.body_bytes).expect("fixture body fits")
}

fn fleet(
    limits: Limits,
) -> (
    Courier,
    ManagedSessionId,
    ManagedSessionId,
    ManagedSessionId,
) {
    let mut courier = Courier::new(limits);
    let a = ManagedSessionId::now();
    let b = ManagedSessionId::now();
    let c = ManagedSessionId::now();
    for session in [a, b, c] {
        assert!(courier.session_started(session));
    }
    (courier, a, b, c)
}

fn round(
    courier: &mut Courier,
    owner: ManagedSessionId,
    room: RoomId,
    target: ManagedSessionId,
) -> Receipt {
    courier
        .room_ask(owner, room, target, MessageId::now(), body("question"), NOW)
        .expect("round admitted")
}

#[test]
fn six_alternating_rounds_are_fresh_and_the_last_reply_survives_the_round_ceiling() {
    let (mut courier, a, b, _) = fleet(Limits::INITIAL);
    let room = courier.room_open(a, &[a, b], DEADLINE, NOW).expect("room");
    let mut speaker = a;
    for number in 1..=Limits::INITIAL.room_rounds {
        let target = if speaker == a { b } else { a };
        let receipt = round(&mut courier, speaker, room, target);
        let ask = courier.receive(target, NOW).expect("one ask");
        assert_eq!(ask.room_id, Some(room));
        assert_eq!(ask.hop_count, 1);
        assert_eq!(ask.visited.as_slice(), &[speaker]);
        courier
            .answer(
                target,
                ask.message_id,
                MessageId::now(),
                body("answer"),
                NOW,
            )
            .expect("exact reply");
        assert_eq!(courier.charged_bytes(), "answer".len());
        assert_eq!(
            courier
                .room_view(a, room, NOW)
                .expect("retained room")
                .rounds,
            number
        );
        assert_eq!(
            courier.room_transfer(a, room, target, NOW),
            Err(RoomError::Busy)
        );
        let refusal = courier.room_ask(speaker, room, target, MessageId::now(), body("extra"), NOW);
        assert_eq!(
            refusal,
            Err(if number == Limits::INITIAL.room_rounds {
                RoomError::RoundBound
            } else {
                RoomError::Busy
            })
        );
        let reply = courier
            .receive_matching(
                speaker,
                Some(target),
                Some(CallRef {
                    call_id: receipt.call_id,
                    ask: receipt.message_id,
                }),
                NOW,
            )
            .expect("including the final answer");
        assert_eq!(reply.room_id, Some(room));
        assert_eq!(reply.reply_to, Some(ask.message_id));
        assert_eq!(reply.body.as_str(), "answer");
        assert_eq!(courier.charged_bytes(), 0);
        assert_eq!(courier.active_calls(), 0);
        courier
            .room_transfer(a, room, target, NOW)
            .expect("explicit transfer after receipt");
        speaker = target;
    }
    assert_eq!(
        courier.room_ask(speaker, room, b, MessageId::now(), body("seventh"), NOW),
        Err(RoomError::RoundBound)
    );
    assert_eq!(
        courier
            .room_view(a, room, NOW)
            .expect("completed metadata persists")
            .in_flight,
        None
    );
    assert_eq!(courier.next_deadline(), Some(DEADLINE));
    courier.sweep(DEADLINE);
    assert_eq!(
        courier.room_view(a, room, DEADLINE),
        Err(RoomError::Unknown(room))
    );
    assert_eq!(courier.next_deadline(), None);
}

#[test]
fn owner_speaker_participants_and_an_active_round_are_independent_authorities() {
    let (mut courier, a, b, c) = fleet(Limits::INITIAL);
    let outsider = ManagedSessionId::now();
    courier.session_started(outsider);
    let room = courier
        .room_open(a, &[a, b, c], DEADLINE, NOW)
        .expect("room");
    assert_eq!(
        courier.room_view(outsider, room, NOW),
        Err(RoomError::NotParticipant(outsider))
    );
    assert_eq!(
        courier.room_transfer(b, room, c, NOW),
        Err(RoomError::NotOwner)
    );
    assert_eq!(
        courier.room_transfer(a, room, outsider, NOW),
        Err(RoomError::NotParticipant(outsider))
    );
    assert_eq!(courier.room_close(b, room), Err(RoomError::NotOwner));
    assert_eq!(
        courier.room_ask(b, room, a, MessageId::now(), body("wrong speaker"), NOW),
        Err(RoomError::NotSpeaker)
    );
    assert_eq!(
        courier.room_ask(
            a,
            room,
            outsider,
            MessageId::now(),
            body("wrong target"),
            NOW
        ),
        Err(RoomError::NotParticipant(outsider))
    );
    courier
        .room_transfer(a, room, b, NOW)
        .expect("owner selects speaker");
    let receipt = round(&mut courier, b, room, c);
    assert_eq!(courier.room_transfer(a, room, a, NOW), Err(RoomError::Busy));
    assert_eq!(
        courier.room_ask(b, room, a, MessageId::now(), body("concurrent"), NOW),
        Err(RoomError::Busy)
    );
    let ask = courier.receive(c, NOW).expect("ask");
    let mut impostor = CallEnvelope::reply(&ask, body("wrong source"), DEADLINE);
    impostor.source = a;
    assert_eq!(
        courier.send(impostor, NOW),
        Err(Refusal::WrongSource { expected: c })
    );
    courier
        .answer(c, receipt.message_id, MessageId::now(), body("answer"), NOW)
        .expect("reply");
    assert_eq!(
        courier.answer(
            c,
            receipt.message_id,
            MessageId::now(),
            body("duplicate"),
            NOW
        ),
        Err(Refusal::AlreadyReplied(receipt.call_id))
    );
    courier.receive(b, NOW).expect("answer consumed");
    courier
        .room_transfer(a, room, a, NOW)
        .expect("next explicit speaker");
}

#[test]
fn direct_room_tags_cannot_bypass_room_authority_or_round_accounting() {
    let (mut courier, a, b, _) = fleet(Limits::INITIAL);
    let room = courier.room_open(a, &[a, b], DEADLINE, NOW).expect("room");
    let mut forged = CallEnvelope::ask(a, b, body("bypass"), DEADLINE);
    forged.room_id = Some(room);
    assert_eq!(courier.send(forged, NOW), Err(Refusal::RoomsClosed));
    assert_eq!(courier.room_view(a, room, NOW).expect("room").rounds, 0);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(courier.active_calls(), 0);
}

#[test]
fn membership_deadline_and_room_count_are_bounded_before_state_is_retained() {
    let (mut courier, a, b, c) = fleet(Limits {
        active_calls: 2,
        ..Limits::INITIAL
    });
    let other = ManagedSessionId::now();
    assert_eq!(
        courier.room_open(a, &[a], DEADLINE, NOW),
        Err(RoomError::ParticipantBound)
    );
    assert_eq!(
        courier.room_open(a, &[a, b, c, other], DEADLINE, NOW),
        Err(RoomError::ParticipantBound)
    );
    assert_eq!(
        courier.room_open(a, &[a, a], DEADLINE, NOW),
        Err(RoomError::DuplicateParticipant(a))
    );
    assert_eq!(
        courier.room_open(a, &[a, other], DEADLINE, NOW),
        Err(RoomError::NotLive(other))
    );
    assert_eq!(
        courier.room_open(a, &[b, c], DEADLINE, NOW),
        Err(RoomError::NotParticipant(a))
    );
    assert_eq!(
        courier.room_open(a, &[a, b], NOW, NOW),
        Err(RoomError::Deadline)
    );
    assert_eq!(
        courier.room_open(
            a,
            &[a, b],
            NOW.plus(Limits::INITIAL.max_deadline_millis + 1),
            NOW
        ),
        Err(RoomError::Deadline)
    );
    assert!(courier.rooms.is_empty());
    let first = courier
        .room_open(a, &[a, b], DEADLINE, NOW)
        .expect("first room");
    courier
        .room_open(a, &[a, c], DEADLINE, NOW)
        .expect("second room");
    assert_eq!(
        courier.room_open(a, &[a, b], DEADLINE, NOW),
        Err(RoomError::Full)
    );
    courier
        .room_close(a, first)
        .expect("close frees the structural slot");
    courier
        .room_open(a, &[a, b], DEADLINE, NOW)
        .expect("reused allowance");
    assert_eq!(courier.rooms.len(), 2);
    assert_eq!(courier.charged_bytes(), 0);
    assert_eq!(courier.active_calls(), 0);
}

#[test]
fn a_refused_mailbox_admission_consumes_neither_a_round_nor_a_room_call() {
    let (mut courier, a, b, c) = fleet(Limits {
        mailbox_envelopes: 1,
        ..Limits::INITIAL
    });
    let room = courier.room_open(a, &[a, b], DEADLINE, NOW).expect("room");
    let unrelated = CallEnvelope::tell(c, b, body("full"), DEADLINE);
    courier.send(unrelated.clone(), NOW).expect("fill mailbox");
    let before = courier.room_view(a, room, NOW).expect("room");
    assert_eq!(
        courier.room_ask(a, room, b, MessageId::now(), body("question"), NOW),
        Err(RoomError::Envelope(Refusal::MailboxEnvelopes {
            session: b,
            ceiling: 1
        }))
    );
    assert_eq!(courier.room_view(a, room, NOW).expect("room"), before);
    assert_eq!(courier.active_calls(), 0);
    assert_eq!(courier.charged_bytes(), unrelated.body.len());
}

#[test]
fn closing_or_expiring_a_room_releases_each_call_stage_but_preserves_unrelated_mail() {
    for expiry in [false, true] {
        for phase in 0..3 {
            let (mut courier, a, b, c) = fleet(Limits::INITIAL);
            let room = courier.room_open(a, &[a, b], DEADLINE, NOW).expect("room");
            let keep_a = CallEnvelope::tell(c, a, body("keep a"), UnixMillis(2_000));
            let keep_b = CallEnvelope::tell(c, b, body("keep b"), UnixMillis(2_000));
            courier.send(keep_a.clone(), NOW).expect("unrelated mail");
            let receipt = round(&mut courier, a, room, b);
            if phase > 0 {
                courier.receive(b, NOW).expect("room ask consumed");
            }
            if phase == 2 {
                courier
                    .answer(b, receipt.message_id, MessageId::now(), body("answer"), NOW)
                    .expect("room reply");
            }
            courier.send(keep_b.clone(), NOW).expect("unrelated mail");
            let removed_bytes = match phase {
                0 => "question".len(),
                1 => 0,
                _ => "answer".len(),
            };
            if expiry {
                let swept = courier.sweep(DEADLINE);
                assert_eq!(swept.calls, vec![receipt.call_id]);
                assert_eq!(swept.bytes, removed_bytes);
            } else {
                let released = courier.room_close(a, room).expect("owner closes room");
                assert_eq!(released.calls, 1);
                assert_eq!(released.bytes, removed_bytes);
            }
            assert_eq!(courier.active_calls(), 0);
            assert_eq!(
                courier.charged_bytes(),
                keep_a.body.len() + keep_b.body.len()
            );
            assert_eq!(
                courier.receive(a, NOW).expect("kept a").message_id,
                keep_a.message_id
            );
            assert_eq!(
                courier.receive(b, NOW).expect("kept b").message_id,
                keep_b.message_id
            );
            assert_eq!(courier.charged_bytes(), 0);
            assert_eq!(
                courier.state_of(receipt.message_id),
                Some(if phase == 2 {
                    DeliveryState::Replied
                } else if expiry {
                    DeliveryState::Expired
                } else {
                    DeliveryState::Cancelled
                })
            );
        }
    }
}

#[test]
fn an_uninvolved_participant_exit_closes_the_room_without_dropping_other_rooms() {
    let (mut courier, a, b, c) = fleet(Limits::INITIAL);
    let room = courier
        .room_open(a, &[a, b, c], DEADLINE, NOW)
        .expect("three participants");
    let survivor = courier
        .room_open(a, &[a, b], DEADLINE, NOW)
        .expect("independent room");
    let closed_call = round(&mut courier, a, room, b);
    let kept_call = round(&mut courier, a, survivor, b);
    let released = courier.session_ended(c);
    assert_eq!(released.calls, 1);
    assert_eq!(released.envelopes, 1);
    assert_eq!(courier.active_calls(), 1);
    assert_eq!(courier.charged_bytes(), "question".len());
    assert_eq!(
        courier.room_view(a, room, NOW),
        Err(RoomError::Unknown(room))
    );
    assert!(courier.room_view(a, survivor, NOW).is_ok());
    let kept = courier.receive(b, NOW).expect("the other room's ask");
    assert_eq!(kept.message_id, kept_call.message_id);
    assert_eq!(
        courier.state_of(closed_call.message_id),
        Some(DeliveryState::Cancelled)
    );
}

#[test]
fn cancelled_and_abandoned_rounds_count_toward_the_ceiling_and_room_close_removes_notices() {
    let (mut courier, a, b, _) = fleet(Limits::INITIAL);
    let room = courier.room_open(a, &[a, b], DEADLINE, NOW).expect("room");
    for number in 0..Limits::INITIAL.room_rounds {
        let receipt = round(&mut courier, a, room, b);
        if number % 2 == 0 {
            courier
                .cancel_call(a, receipt.call_id, MessageId::now(), NOW)
                .expect("explicit cancellation");
        } else {
            courier.abandon_call(
                a,
                CallRef {
                    call_id: receipt.call_id,
                    ask: receipt.message_id,
                },
            );
        }
        assert_eq!(
            courier.room_view(a, room, NOW).expect("room").in_flight,
            None
        );
        assert_eq!(courier.active_calls(), 0);
        assert_eq!(courier.charged_bytes(), 0);
    }
    assert_eq!(
        courier.room_ask(a, room, b, MessageId::now(), body("seventh"), NOW),
        Err(RoomError::RoundBound)
    );
    assert_eq!(courier.waiting(b), Some(3));
    let notice = courier.receive(b, NOW).expect("one cancellation notice");
    assert_eq!(notice.kind, CallKind::Cancel);
    assert_eq!(notice.room_id, Some(room));
    assert_eq!(courier.room_close(a, room).expect("close").envelopes, 2);
    assert_eq!(courier.waiting(b), Some(0));
}

#[test]
fn reactivating_the_same_session_does_not_revive_its_old_room_or_call_authority() {
    let (mut courier, a, b, _) = fleet(Limits::INITIAL);
    let old_room = courier
        .room_open(a, &[a, b], DEADLINE, NOW)
        .expect("old room");
    let old_call = round(&mut courier, a, old_room, b);
    let old_ask = courier.receive(b, NOW).expect("old round delivered");
    courier.session_ended(a);
    assert!(
        courier.session_started(a),
        "reactivate a fresh mailbox under the same session identity"
    );
    let new_room = courier
        .room_open(a, &[a, b], DEADLINE, NOW)
        .expect("new room");
    let new_call = round(&mut courier, a, new_room, b);
    assert_ne!(new_room, old_room);
    assert_eq!(
        courier.room_close(a, old_room),
        Err(RoomError::Unknown(old_room))
    );
    assert_eq!(
        courier.send(
            CallEnvelope::reply(&old_ask, body("stale answer"), DEADLINE),
            NOW
        ),
        Err(Refusal::NoSuchCall(old_call.call_id))
    );
    courier.abandon_call(
        a,
        CallRef {
            call_id: old_call.call_id,
            ask: old_call.message_id,
        },
    );
    assert_eq!(courier.active_calls(), 1);
    let heard = courier.receive(b, NOW).expect("new room's ask remains");
    assert_eq!(heard.message_id, new_call.message_id);
    assert_eq!(heard.room_id, Some(new_room));
    assert_eq!(
        courier
            .room_view(a, new_room, NOW)
            .expect("new room")
            .rounds,
        1
    );
}
