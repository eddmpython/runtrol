use std::future::{Future as _, poll_fn};
use std::task::Poll;
use std::time::Instant;

use runtrol_ipc::transport::{Listener, connect};
use runtrol_provider::TerminalId;

use super::*;
use crate::runtime_terminal::view_control_tests::Fixture;

async fn pair(view: &TerminalView) -> (Connection, Connection) {
    #[cfg(windows)]
    let address = format!(
        r"\\.\pipe\runtrol-root-output-{}-{}",
        view.hosted.id,
        TerminalId::now()
    );
    #[cfg(not(windows))]
    let address = view
        .hosted
        .workspace
        .as_std_path()
        .join(format!("root-output-{}", TerminalId::now()))
        .to_str()
        .expect("UTF-8 fixture socket")
        .to_owned();
    let mut listener = Listener::bind(&address)
        .await
        .expect("bind the exact fixture endpoint");
    let (client, server) = tokio::join!(connect(&address), listener.accept());
    (
        server.expect("accept fixture connection"),
        client.expect("connect fixture reader"),
    )
}

async fn frame_is_refused(view: &TerminalView, method: RuntimeMethod) {
    let (mut server, mut client) = pair(view).await;
    let result = send_root_output(
        &mut server,
        view,
        method,
        &serde_json::json!({"fixture":true}),
    )
    .await;
    drop(server);
    let received = client
        .recv()
        .await
        .expect("read the closed fixture transport");
    assert!(result.is_err());
    assert!(
        received.is_none(),
        "no frame reaches the transport after root authority ends"
    );
}

#[derive(Clone, Copy)]
enum OutputPath {
    Live,
    ExitDrain,
    Lagged,
}

async fn expires_while_waiting_for_output(path: OutputPath) {
    let mut fixture = Fixture::new().await;
    fixture.enable_output().await;
    let mut view = fixture.another_terminal_view(1).await;
    let proof = Arc::clone(view.root_proof());
    let terminal = view.hosted.terminal.clone();
    let mut observer = terminal.attach().await;
    let (lag_sender, lag_receiver) = tokio::sync::broadcast::channel(1);
    if matches!(path, OutputPath::Lagged) {
        view.attachment.live = lag_receiver;
    }
    let (mut server, mut client) = pair(&view).await;
    let mut relay = Box::pin(relay_terminal(&mut server, &fixture.composed, view));
    poll_fn(|context| {
        assert!(
            relay.as_mut().poll(context).is_pending(),
            "the authorized relay waits for its next event"
        );
        Poll::Ready(())
    })
    .await;
    if matches!(path, OutputPath::Lagged) {
        for sequence in 1..=2 {
            lag_sender
                .send(runtrol_core::terminal::OutputChunk {
                    sequence,
                    bytes: b"fixture-output".to_vec().into(),
                })
                .expect("overflow the selected view's bounded receiver");
        }
    } else {
        terminal
            .feed(b"fixture-output".to_vec())
            .expect("feed synthetic terminal bytes");
        observer
            .live
            .recv()
            .await
            .expect("the synthetic chunk is published");
    }
    if matches!(path, OutputPath::ExitDrain) {
        terminal
            .end_feed(Some(0))
            .expect("end the exact synthetic feed");
        if observer.exited.borrow().exit_code.is_none() {
            observer
                .exited
                .changed()
                .await
                .expect("the exit is published after output");
        }
    }
    proof.set_test_completion(
        Instant::now()
            .checked_sub(Duration::from_secs(2))
            .expect("expired fixture proof"),
    );
    tokio::time::timeout(Duration::from_secs(1), &mut relay)
        .await
        .expect("the relay refuses stale output");
    drop(relay);
    drop(server);
    let received = client.recv().await.expect("read closed fixture transport");
    let checks = proof.test_output_checks();
    drop(client);
    drop(observer);
    drop(lag_sender);
    drop(terminal);
    drop(proof);
    fixture.close().await;
    assert_eq!(
        checks,
        usize::from(!matches!(path, OutputPath::ExitDrain)),
        "completion refreshes all authority before draining; ordinary sends recheck their last output boundary"
    );
    assert!(
        received.is_none(),
        "the expired proof authorizes no output frame"
    );
}

#[tokio::test]
async fn normal_output_rechecks_freshness_after_awaiting_the_next_chunk() {
    expires_while_waiting_for_output(OutputPath::Live).await;
}

#[tokio::test]
async fn exit_drain_rechecks_freshness_before_sending_queued_output() {
    expires_while_waiting_for_output(OutputPath::ExitDrain).await;
}

#[tokio::test]
async fn lag_replacement_rechecks_freshness_after_attaching_a_new_snapshot() {
    expires_while_waiting_for_output(OutputPath::Lagged).await;
}

#[tokio::test]
async fn a_real_directory_denial_stops_output_while_its_old_timestamp_is_still_recent() {
    let fixture = Fixture::new().await;
    let original = fixture.view.hosted.workspace.as_std_path().to_owned();
    let moved = original.with_file_name("moved-root");
    std::fs::rename(&original, &moved).expect("move the fixture's approved root");
    let proof = Arc::clone(fixture.view.root_proof());
    let result = tokio::task::spawn_blocking(move || proof.checked_now())
        .await
        .expect("observe the changed root");
    std::fs::rename(&moved, &original).expect("restore the exact fixture root");
    assert_eq!(result, Err(RootCheckFailure::Denied));
    frame_is_refused(&fixture.view, RuntimeMethod::TerminalsOutput).await;
    frame_is_refused(&fixture.view, RuntimeMethod::TerminalsLagged).await;
    fixture.close().await;
}

#[tokio::test]
async fn a_current_proof_sends_the_exact_output_frame() {
    let fixture = Fixture::new().await;
    let (mut server, mut client) = pair(&fixture.view).await;
    let body = serde_json::json!({"fixture": "unchanged"});
    send_root_output(
        &mut server,
        &fixture.view,
        RuntimeMethod::TerminalsOutput,
        &body,
    )
    .await
    .expect("send an authorized frame");
    let bytes = client
        .recv()
        .await
        .expect("read the frame")
        .expect("a frame arrives");
    let frame: serde_json::Value =
        serde_json::from_slice(&bytes).expect("decode the synthetic envelope");
    assert_eq!(frame.get("params"), Some(&body));
    drop(server);
    drop(client);
    fixture.close().await;
}

#[tokio::test]
async fn an_admitted_view_refreshes_an_aged_shared_proof_without_waiting_a_new_interval() {
    refresh_aged_proof(false).await;
}

#[tokio::test]
async fn an_input_response_recomputes_refresh_even_when_the_completion_notification_was_missed() {
    refresh_aged_proof(true).await;
}

async fn refresh_aged_proof(after_request: bool) {
    let mut fixture = Fixture::new().await;
    fixture.enable_output().await;
    let view = fixture.another_terminal_view(1).await;
    let proof = Arc::clone(view.root_proof());
    let calls = proof.test_calls();
    let mut changed = proof.subscribe();
    let (mut server, mut client) = pair(&view).await;
    let mut relay = Box::pin(relay_terminal(&mut server, &fixture.composed, view));
    if after_request {
        poll_fn(|context| {
            assert!(relay.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    // No notification is emitted: an inbound handler can replace its receiver after a check completed.
    // The proof remains authorized, but its refresh is already due before a new 500ms interval would fire.
    let aged = Instant::now()
        .checked_sub(Duration::from_millis(650))
        .expect("an aged, still-authorized fixture proof");
    proof.set_test_completion(aged);
    if after_request {
        client
            .send(br#"{"jsonrpc":"2.0","id":1,"method":"fixture/unknown","params":{}}"#)
            .await
            .expect("send through the dedicated inbound branch");
    }
    let refreshed = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            tokio::select! {
                outcome = &mut relay => panic!("fresh root relay ended before refresh: {outcome:?}"),
                result = changed.changed() => {
                    result.expect("the shared checker publishes completion");
                    if proof.fresh().is_ok_and(|completed| completed > aged) {
                        assert_eq!(proof.test_calls(), calls + 1);
                        break;
                    }
                }
                result = client.recv(), if after_request => {
                    assert!(result.expect("read the control response").is_some());
                }
            }
        }
    })
    .await;
    drop(relay);
    drop(server);
    drop(client);
    drop(changed);
    drop(proof);
    fixture.close().await;
    assert!(
        refreshed.is_ok(),
        "refresh follows proof age instead of a view-local timer"
    );
}

#[tokio::test]
async fn terminal_completion_drains_queued_output_and_preserves_failure_on_both_exit_paths() {
    for already_ended in [false, true] {
        completed_output(already_ended, false).await;
    }
}

#[tokio::test]
async fn terminal_completion_preserves_the_lag_boundary_on_both_exit_paths() {
    for already_ended in [false, true] {
        completed_output(already_ended, true).await;
    }
}

#[tokio::test]
async fn terminal_completion_cannot_bypass_a_queued_grant_revocation() {
    let mut fixture = Fixture::new().await;
    fixture.enable_output().await;
    let view = fixture.another_terminal_view(1).await;
    let key = view.authority.key;
    let terminal = view.hosted.terminal.clone();
    let mut observer = terminal.attach().await;
    let (mut server, mut client) = pair(&view).await;
    let mut relay = Box::pin(relay_terminal(&mut server, &fixture.composed, view));
    poll_fn(|context| {
        assert!(relay.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    terminal
        .feed(b"withheld final frame".to_vec())
        .expect("feed one accepted raw frame");
    observer
        .live
        .recv()
        .await
        .expect("the frame reaches the raw ring");
    terminal
        .end_feed(Some(0))
        .expect("complete the exact owner");
    while observer.exited.borrow().exit_code.is_none() {
        observer
            .exited
            .changed()
            .await
            .expect("the authority drains");
    }
    let mut row = fixture
        .composed
        .integration_authority
        .row(key)
        .expect("the current integration")
        .as_ref()
        .clone();
    row.grant_generation += 1;
    row.scopes.clear();
    fixture
        .composed
        .integration_authority
        .publish_committed(key, row)
        .expect("withdraw output before completion is delivered");
    tokio::time::timeout(Duration::from_secs(2), &mut relay)
        .await
        .expect("the revoked relay ends");
    drop(relay);
    drop(server);
    let received = client.recv().await.expect("read the closed transport");
    drop(client);
    drop(observer);
    drop(terminal);
    fixture.close().await;
    assert!(
        received.is_none(),
        "completion cannot skip a simultaneously queued authority change"
    );
}

async fn completed_output(already_ended: bool, lagged: bool) {
    let mut fixture = Fixture::new().await;
    fixture.enable_output().await;
    let mut view = fixture.another_terminal_view(1).await;
    let terminal = view.hosted.terminal.clone();
    let mut observer = terminal.attach().await;
    let (completed, completion) =
        watch::channel(runtrol_core::terminal::TerminalCompletion::default());
    view.attachment.exited = completion;
    let (lag_sender, lag_receiver) = tokio::sync::broadcast::channel(1);
    if lagged {
        view.attachment.live = lag_receiver;
    }
    let (mut server, mut client) = pair(&view).await;
    let mut relay = Box::pin(relay_terminal(&mut server, &fixture.composed, view));
    if !already_ended {
        poll_fn(|context| {
            assert!(relay.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    terminal
        .feed(b"last raw frame".to_vec())
        .expect("publish the final frame");
    observer
        .live
        .recv()
        .await
        .expect("the final raw frame reaches the ring");
    terminal.end_feed(Some(0)).expect("end the exact feed");
    while observer.exited.borrow().exit_code.is_none() {
        observer
            .exited
            .changed()
            .await
            .expect("Core completes its authority");
    }
    if lagged {
        for sequence in 1..=2 {
            lag_sender
                .send(runtrol_core::terminal::OutputChunk {
                    sequence,
                    bytes: b"lag fixture".to_vec().into(),
                })
                .expect("overflow only the selected viewer");
        }
    }
    completed.send_replace(runtrol_core::terminal::TerminalCompletion {
        exit_code: Some(0),
        failure: Some(runtrol_core::terminal::TerminalFailure::OutputReadFailed),
    });
    tokio::time::timeout(Duration::from_secs(2), &mut relay)
        .await
        .expect("the completed ring drains");
    drop(relay);
    drop(server);
    let mut frames = Vec::new();
    while let Some(bytes) = client
        .recv()
        .await
        .expect("read the exact completion frames")
    {
        frames.push(
            serde_json::from_slice::<runtrol_runtime_protocol::JsonRpcNotification>(&bytes)
                .expect("a public notification"),
        );
    }
    drop(client);
    drop(observer);
    drop(terminal);
    drop(lag_sender);
    fixture.close().await;
    assert_completion_frames(&frames, lagged);
}

fn assert_completion_frames(
    frames: &[runtrol_runtime_protocol::JsonRpcNotification],
    lagged: bool,
) {
    assert_eq!(frames.len(), 2);
    let first = frames
        .first()
        .expect("the final output precedes completion");
    if lagged {
        assert_eq!(first.method, "terminals/lagged");
        let replacement: TerminalLaggedNotification =
            serde_json::from_value(first.params.clone()).expect("an explicit lag replacement");
        assert!(replacement.lost_chunks > 0);
        assert!(
            String::from_utf8(base64ct::Base64::decode_vec(&replacement.screen_base64).unwrap())
                .unwrap()
                .contains("last raw frame")
        );
    } else {
        assert_eq!(first.method, "terminals/output");
        let output: TerminalOutputNotification =
            serde_json::from_value(first.params.clone()).expect("exact raw output");
        assert_eq!(
            base64ct::Base64::decode_vec(&output.bytes_base64).unwrap(),
            b"last raw frame"
        );
    }
    let last = frames.last().expect("completion follows output");
    assert_eq!(last.method, "terminals/exited");
    let exit: TerminalExitedNotification =
        serde_json::from_value(last.params.clone()).expect("the completion contract");
    assert_eq!(exit.exit_code, 0);
    assert_eq!(
        exit.failure,
        Some(runtrol_runtime_protocol::TerminalFailure::OutputReadFailed)
    );
}
