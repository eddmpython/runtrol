//! Room wire and connection lifetimes, with the production activation adapter around the prototype core.

use std::future::{Future, poll_fn};
use std::task::Poll;

use runtrol_courier::wire::{Answer, Request};
use runtrol_courier::{CallEnvelope, Limits, MessageId, RoomId};

use super::commands::{Fleet, admission, body, fleet, now};

fn wire(request: Request) -> Request {
    let bytes = serde_json::to_vec(&request).expect("encode command");
    drop(request);
    assert!(bytes.len() <= runtrol_courier::wire::MAX_FRAME_BYTES);
    serde_json::from_slice(&bytes).expect("decode exact command")
}

async fn open(fleet: &Fleet) -> RoomId {
    let Answer::Room { room } = fleet
        .gate
        .command(
            fleet.alpha,
            wire(Request::RoomOpen {
                participants: vec![fleet.alpha, fleet.bravo],
                deadline: now().plus(10_000),
            }),
        )
        .await
    else {
        panic!("open the room");
    };
    room.id
}

fn ask(room: RoomId, fleet: &Fleet, message_id: MessageId, timeout_ms: u64) -> Request {
    wire(Request::RoomAsk {
        room,
        target: fleet.bravo,
        message_id,
        body: body("opaque 질문"),
        timeout_ms,
    })
}

async fn pending(future: std::pin::Pin<&mut impl Future<Output = Answer>>) {
    let mut future = future;
    poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "the admitted ask is waiting"
        );
        Poll::Ready(())
    })
    .await;
}

async fn inbox(fleet: &Fleet, session: runtrol_courier::ManagedSessionId) -> Option<CallEnvelope> {
    let Answer::Received { envelope } = fleet
        .gate
        .command(
            session,
            wire(Request::Receive {
                source: None,
                call: None,
                timeout_ms: 0,
            }),
        )
        .await
    else {
        panic!("read one mailbox envelope");
    };
    envelope
}

#[tokio::test]
async fn six_explicit_wire_rounds_keep_the_final_reply_and_then_refuse_a_seventh() {
    let fleet = fleet().await;
    let room = open(&fleet).await;
    for round in 0..Limits::INITIAL.room_rounds {
        let (speaker, target) = if round % 2 == 0 {
            (fleet.alpha, fleet.bravo)
        } else {
            (fleet.bravo, fleet.alpha)
        };
        assert!(matches!(
            fleet
                .gate
                .command(fleet.alpha, wire(Request::RoomTransfer { room, speaker }))
                .await,
            Answer::Room { .. }
        ));
        let admitted = admission(&fleet.gate, speaker).await;
        let mut cleanup = None;
        let request = wire(Request::RoomAsk {
            room,
            target,
            message_id: MessageId::now(),
            body: body("질문\nexact bytes"),
            timeout_ms: 1_000,
        });
        let waiting = fleet.gate.command_owned(admitted, request, &mut cleanup);
        tokio::pin!(waiting);
        pending(waiting.as_mut()).await;
        let ask = inbox(&fleet, target).await.expect("receive the round");
        assert_eq!(ask.room_id, Some(room));
        assert_eq!(ask.source, speaker);
        assert_eq!(ask.hop_count, 1, "each explicit round starts a fresh chain");
        assert!(matches!(
            fleet
                .gate
                .command(
                    target,
                    wire(Request::Reply {
                        message: ask.message_id,
                        message_id: MessageId::now(),
                        body: body("응답\nfinal bytes"),
                    })
                )
                .await,
            Answer::Accepted { .. }
        ));
        let Answer::Room {
            room: before_receive,
        } = fleet
            .gate
            .command(speaker, Request::RoomInspect { room })
            .await
        else {
            panic!("inspect the retained final reply");
        };
        assert!(before_receive.in_flight.is_some());
        let Answer::Received {
            envelope: Some(reply),
        } = waiting.await
        else {
            panic!("exact reply reached its ask");
        };
        assert_eq!(reply.reply_to, Some(ask.message_id));
        assert_eq!(reply.body.as_str(), "응답\nfinal bytes");
    }
    let mut cleanup = None;
    let admitted = admission(&fleet.gate, fleet.bravo).await;
    assert!(matches!(
        fleet
            .gate
            .command_owned(
                admitted,
                wire(Request::RoomAsk {
                    room,
                    target: fleet.alpha,
                    message_id: MessageId::now(),
                    body: body("seventh"),
                    timeout_ms: 1_000,
                }),
                &mut cleanup
            )
            .await,
        Answer::Refused { .. }
    ));
    assert!(cleanup.is_none());
    assert!(matches!(
        fleet
            .gate
            .command(fleet.alpha, wire(Request::RoomClose { room }))
            .await,
        Answer::RoomClosed { .. }
    ));
}

#[tokio::test]
async fn room_refusals_cannot_cancel_the_original_wait_or_spoof_its_source() {
    let fleet = fleet().await;
    let room = open(&fleet).await;
    let admitted = admission(&fleet.gate, fleet.alpha).await;
    let message = MessageId::now();
    let mut cleanup = None;
    let waiting =
        fleet
            .gate
            .command_owned(admitted, ask(room, &fleet, message, 1_000), &mut cleanup);
    tokio::pin!(waiting);
    pending(waiting.as_mut()).await;
    for request in [
        ask(room, &fleet, message, 1_000),
        ask(room, &fleet, MessageId::now(), 0),
        ask(
            room,
            &fleet,
            MessageId::now(),
            Limits::INITIAL.max_deadline_millis + 1,
        ),
    ] {
        let mut false_cleanup = None;
        assert!(matches!(
            fleet
                .gate
                .command_owned(admitted, request, &mut false_cleanup)
                .await,
            Answer::Refused { .. }
        ));
        assert!(
            false_cleanup.is_none(),
            "only an admitted round has cleanup authority"
        );
    }
    let ask = inbox(&fleet, fleet.bravo)
        .await
        .expect("original ask was preserved");
    assert_eq!(ask.message_id, message);
    assert!(matches!(
        fleet
            .gate
            .command(
                fleet.charlie,
                Request::Reply {
                    message,
                    message_id: MessageId::now(),
                    body: body("not the target"),
                }
            )
            .await,
        Answer::Refused { .. }
    ));
    let mut forged = serde_json::to_value(Request::RoomInspect { room }).expect("wire value");
    forged
        .as_object_mut()
        .expect("serialized room request is an object")
        .insert(
            "source".to_owned(),
            serde_json::to_value(fleet.alpha).expect("identity"),
        );
    assert!(
        serde_json::from_value::<Request>(forged).is_err(),
        "room authority cannot be supplied in the wire"
    );
    assert!(matches!(
        fleet
            .gate
            .command(
                fleet.bravo,
                Request::Reply {
                    message,
                    message_id: MessageId::now(),
                    body: body("right target"),
                }
            )
            .await,
        Answer::Accepted { .. }
    ));
    assert!(matches!(
        waiting.await,
        Answer::Received { envelope: Some(_) }
    ));
}

#[tokio::test]
async fn every_room_command_refuses_a_hello_from_an_older_activation() {
    let fleet = fleet().await;
    let stale = admission(&fleet.gate, fleet.alpha).await;
    fleet
        .gate
        .set_dialogue(fleet.alpha_terminal, false)
        .await
        .expect("disable");
    fleet
        .gate
        .set_dialogue(fleet.alpha_terminal, true)
        .await
        .expect("new lifetime");
    let room = open(&fleet).await;
    for request in [
        Request::RoomOpen {
            participants: vec![fleet.alpha, fleet.bravo],
            deadline: now().plus(1_000),
        },
        Request::RoomInspect { room },
        Request::RoomTransfer {
            room,
            speaker: fleet.bravo,
        },
        Request::RoomClose { room },
        ask(room, &fleet, MessageId::now(), 1_000),
    ] {
        let mut cleanup = None;
        assert!(matches!(
            fleet
                .gate
                .command_owned(stale, wire(request), &mut cleanup)
                .await,
            Answer::Refused { .. }
        ));
        assert!(cleanup.is_none());
    }
    let Answer::Room { room } = fleet
        .gate
        .command(fleet.alpha, Request::RoomInspect { room })
        .await
    else {
        panic!("new lifetime's room remains");
    };
    assert_eq!(room.speaker, fleet.alpha);
    assert_eq!(room.rounds, 0);
}

#[tokio::test]
async fn timeout_cleanup_releases_only_the_exact_round_and_keeps_unrelated_mail() {
    let fleet = fleet().await;
    let room = open(&fleet).await;
    let unrelated = CallEnvelope::tell(
        fleet.charlie,
        fleet.bravo,
        body("keep me"),
        now().plus(1_000),
    );
    assert!(matches!(
        fleet
            .gate
            .command(
                fleet.charlie,
                Request::Send {
                    envelope: unrelated.clone()
                }
            )
            .await,
        Answer::Accepted { .. }
    ));
    let admitted = admission(&fleet.gate, fleet.alpha).await;
    let mut cleanup = None;
    assert!(matches!(
        fleet
            .gate
            .command_owned(
                admitted,
                ask(room, &fleet, MessageId::now(), 5),
                &mut cleanup
            )
            .await,
        Answer::Received { envelope: None }
    ));
    fleet
        .gate
        .abandon(fleet.alpha, cleanup.expect("accepted round owns cleanup"))
        .await;
    assert_eq!(
        inbox(&fleet, fleet.bravo)
            .await
            .expect("unrelated survives")
            .message_id,
        unrelated.message_id
    );
    assert!(inbox(&fleet, fleet.bravo).await.is_none());
    let Answer::Room { room } = fleet
        .gate
        .command(fleet.alpha, Request::RoomInspect { room })
        .await
    else {
        panic!("room survives bounded round timeout");
    };
    assert_eq!(room.rounds, 1);
    assert!(room.in_flight.is_none());
}

#[tokio::test]
async fn disabling_a_participant_wakes_and_retires_the_room_wait() {
    let fleet = fleet().await;
    let room = open(&fleet).await;
    let admitted = admission(&fleet.gate, fleet.alpha).await;
    let mut cleanup = None;
    {
        let waiting = fleet.gate.command_owned(
            admitted,
            ask(room, &fleet, MessageId::now(), 1_000),
            &mut cleanup,
        );
        tokio::pin!(waiting);
        pending(waiting.as_mut()).await;
        fleet
            .gate
            .set_dialogue(fleet.alpha_terminal, false)
            .await
            .expect("retire the room owner lifetime");
        assert!(matches!(waiting.await, Answer::Refused { .. }));
    }
    fleet
        .gate
        .set_dialogue(fleet.alpha_terminal, true)
        .await
        .expect("new activation");
    let next_room = open(&fleet).await;
    fleet
        .gate
        .abandon(fleet.alpha, cleanup.expect("old round cleanup"))
        .await;
    assert!(matches!(
        fleet
            .gate
            .command(fleet.bravo, Request::RoomInspect { room })
            .await,
        Answer::Refused { .. }
    ));
    assert!(matches!(
        fleet
            .gate
            .command(fleet.alpha, Request::RoomInspect { room: next_room })
            .await,
        Answer::Room { .. }
    ));
}
