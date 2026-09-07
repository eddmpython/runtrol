//! A slow authority retains every byte that determines a query answer within the existing ring bound.

use super::input_tests::{finish, host_recorder};
use super::*;
use std::future::{Future, poll_fn};
use std::task::Poll;

async fn publish_raw(terminal: &Terminal, viewer: &mut Attachment, bytes: &'static [u8]) {
    terminal.shared.take_output(Bytes::from_static(bytes)).await;
    assert_eq!(
        viewer
            .live
            .recv()
            .await
            .expect("a current raw viewer keeps up")
            .bytes
            .as_ref(),
        bytes
    );
}

async fn wait_through(terminal: &Terminal, sequence: u64) {
    let mut projected = terminal.shared.projected.subscribe();
    tokio::time::timeout(Duration::from_secs(2), async {
        while projected.borrow_and_update().through < sequence {
            projected
                .changed()
                .await
                .expect("the host publishes authority progress");
        }
    })
    .await
    .expect("the authority reaches the fixture boundary");
}

#[tokio::test]
async fn a_full_raw_ring_never_discards_the_cursor_before_a_split_query() {
    let (terminal, mut written) = host_recorder();
    let mut viewer = terminal.attach().await;
    let mut fast = terminal.attach().await;
    let stalled = terminal.shared.projector.lock().await;
    // Exhaust the product ring without a cooperative scheduler yield letting the authority catch up.
    let publishing = tokio::task::unconstrained(async {
        publish_raw(&terminal, &mut fast, b"\x1b[7;9H").await;
        for _ in 1..RING_CHUNKS {
            publish_raw(&terminal, &mut fast, b"\x1b[0m").await;
        }
        publish_raw(&terminal, &mut fast, b"\x1b[").await;
        publish_raw(&terminal, &mut fast, b"6n").await;
    });
    tokio::pin!(publishing);
    // One poll either exposes the old overwrite or reaches the lossless ring's exact capacity.
    let completed =
        poll_fn(|context| Poll::Ready(publishing.as_mut().poll(context).is_ready())).await;
    drop(stalled);
    if !completed {
        publishing.await;
    }
    let answer = tokio::time::timeout(Duration::from_secs(2), written.recv()).await;
    finish(&terminal).await;
    assert_eq!(
        answer.expect("the authority catches up").expect("one CPR"),
        b"\x1b[7;9R",
        "SGR and read boundaries cannot reset the cursor observed by CPR"
    );
    assert!(
        matches!(
            viewer.live.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ),
        "an independently slow viewer may lag without weakening authority"
    );
    assert!(
        written.try_recv().is_err(),
        "the split query is answered once"
    );
}

#[tokio::test]
async fn a_checkpoint_never_produces_queries_while_the_host_writer_is_waiting() {
    let (terminal, mut written) = host_recorder();
    let held_input = terminal
        .operation()
        .await
        .expect("hold the exact input lane");
    terminal
        .shared
        .take_output(Bytes::from_static(b"abc\x1b[6n"))
        .await;
    wait_through(&terminal, 1).await;
    terminal
        .shared
        .take_output(Bytes::from_static(b"xy\x1b[6n"))
        .await;
    let attaching = terminal.attach();
    tokio::pin!(attaching);
    let first_poll = poll_fn(|context| Poll::Ready(attaching.as_mut().poll(context))).await;
    let projected_while_writer_waits = terminal.shared.projected.borrow().through;
    drop(held_input);
    if first_poll.is_pending() {
        drop(attaching.await);
    }
    let mut answers = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while answers.len() < b"\x1b[1;4R\x1b[1;6R".len() {
            answers.extend(written.recv().await.expect("the exact host answers"));
        }
    })
    .await
    .expect("both queries complete after input unlocks");
    finish(&terminal).await;
    assert_eq!(
        projected_while_writer_waits, 1,
        "attach is a snapshot consumer, never another query producer"
    );
    assert_eq!(answers, b"\x1b[1;4R\x1b[1;6R");
}

#[tokio::test]
async fn a_waiting_query_writer_leaves_the_entire_raw_ring_available() {
    let (terminal, mut written) = host_recorder();
    let mut viewer = terminal.attach().await;
    let held_input = terminal
        .operation()
        .await
        .expect("hold the exact input order");
    publish_raw(&terminal, &mut viewer, b"abc\x1b[6n").await;
    wait_through(&terminal, 1).await;
    let published = std::cell::Cell::new(0);
    let publishing = tokio::task::unconstrained(async {
        for _ in 0..RING_CHUNKS {
            publish_raw(&terminal, &mut viewer, b"\x1b[0m").await;
            published.set(published.get() + 1);
        }
        publish_raw(&terminal, &mut viewer, b"\x1b[6n").await;
    });
    tokio::pin!(publishing);
    let blocked =
        poll_fn(|context| Poll::Ready(publishing.as_mut().poll(context).is_pending())).await;
    let raw_before_unlock = published.get();
    drop(held_input);
    if blocked {
        publishing.await;
    }
    let mut answers = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while answers.len() < b"\x1b[1;4R\x1b[1;4R".len() {
            answers.extend(written.recv().await.expect("the host writes both answers"));
        }
    })
    .await
    .expect("the query writer resumes");
    finish(&terminal).await;
    assert!(
        blocked,
        "only the first overwrite waits for authority progress"
    );
    assert_eq!(
        raw_before_unlock, RING_CHUNKS,
        "input order cannot consume any raw capacity"
    );
    assert_eq!(answers, b"\x1b[1;4R\x1b[1;4R");
}

#[tokio::test]
async fn exit_releases_a_full_ring_without_waiting_for_a_stalled_authority() {
    let (terminal, mut written) = host_recorder();
    let mut viewer = terminal.attach().await;
    let stalled = terminal.shared.projector.lock().await;
    let publishing = tokio::task::unconstrained(async {
        for _ in 0..RING_CHUNKS {
            publish_raw(&terminal, &mut viewer, b"\x1b[0m").await;
        }
        publish_raw(&terminal, &mut viewer, b"last raw frame\x1b[6n").await;
    });
    tokio::pin!(publishing);
    let blocked =
        poll_fn(|context| Poll::Ready(publishing.as_mut().poll(context).is_pending())).await;
    terminal
        .end_feed(Some(0))
        .expect("the exact synthetic child ends");
    let drained = if blocked {
        tokio::time::timeout(Duration::from_secs(2), publishing)
            .await
            .is_ok()
    } else {
        false
    };
    drop(stalled);
    finish(&terminal).await;
    assert!(
        blocked && drained,
        "process exit wakes the raw producer even while the authority is held"
    );
    assert!(
        written.try_recv().is_err(),
        "an exited CLI is never given a reset cursor answer"
    );
}

#[tokio::test]
async fn a_parser_panic_sends_no_reset_cursor_and_keeps_raw_eof_live() {
    let (terminal, mut written) = host_recorder();
    let mut viewer = terminal.attach().await;
    {
        let mut projector = terminal.shared.projector.lock().await;
        projector.screen = new_screen(PtySize { cols: 4, rows: 2 });
        projector.screen.process("你".as_bytes());
        projector.screen.screen_mut().set_size(2, 1);
    }
    publish_raw(&terminal, &mut viewer, b"\x1b[K\x1b[6n").await;
    let mut exited = terminal.exited();
    tokio::time::timeout(Duration::from_secs(2), async {
        while exited.borrow().exit_code.is_none() {
            exited
                .changed()
                .await
                .expect("the failed authority retains its exact exit watch");
        }
    })
    .await
    .expect("control failure closes input and drains raw EOF");
    assert!(
        written.try_recv().is_err(),
        "no reply names a fabricated reset cursor"
    );
    assert!(terminal.input(b"later").await.is_err());
    assert!(*terminal.shared.output_drained.borrow());
    assert!(!terminal.attach().await.checkpoint_available);
    assert_eq!(
        exited.borrow().failure,
        Some(TerminalFailure::ControlStateLost)
    );
}

#[tokio::test]
async fn completion_waits_for_the_last_authority_frame_after_raw_eof() {
    for broken in [false, true] {
        completion_after_authority(broken).await;
    }
}

async fn completion_after_authority(broken: bool) {
    let (terminal, _written) = host_recorder();
    let mut exited = terminal.exited();
    let mut stalled = terminal.shared.projector.lock().await;
    if broken {
        stalled.screen = new_screen(PtySize { cols: 4, rows: 2 });
        stalled.screen.process("你".as_bytes());
        stalled.screen.screen_mut().set_size(2, 1);
    }
    terminal
        .shared
        .take_output(Bytes::from_static(b"\x1b[Klast frame"))
        .await;
    terminal.end_feed(Some(0)).expect("the exact feed ends");
    let mut drained = terminal.shared.output_drained.subscribe();
    while !*drained.borrow_and_update() {
        drained.changed().await.expect("the raw lane drains");
    }
    let premature = tokio::time::timeout(Duration::from_millis(100), async {
        while exited.borrow().exit_code.is_none() {
            exited.changed().await.expect("the host retains completion");
        }
    })
    .await
    .is_ok();
    drop(stalled);
    finish(&terminal).await;
    assert!(
        !premature,
        "raw EOF cannot finalize completion before the last frame can fail in the authority"
    );
    assert_eq!(terminal.exit(), Some(0));
    assert_eq!(
        exited.borrow().failure,
        broken.then_some(TerminalFailure::ControlStateLost),
        "a zero process code cannot conceal a final authority failure"
    );
}

#[tokio::test]
async fn exit_cancels_a_query_waiting_for_an_input_operation() {
    let (terminal, mut written) = host_recorder();
    let held_input = terminal
        .operation()
        .await
        .expect("a live user input operation");
    terminal
        .shared
        .take_output(Bytes::from_static(b"abc\x1b[6n"))
        .await;
    wait_through(&terminal, 1).await;
    let waiting = terminal.shared.projected.receiver_count();
    finish(&terminal).await;
    let canceled = tokio::time::timeout(Duration::from_secs(2), async {
        while terminal.shared.projected.receiver_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    drop(held_input);
    assert_eq!(
        waiting, 1,
        "the sole query producer is waiting on input order"
    );
    assert!(
        canceled,
        "exit ends the query waiter before the input owner releases its operation"
    );
    assert!(written.try_recv().is_err());
}
