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
        if observer.exited.borrow().is_none() {
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
        checks, 1,
        "the selected relay path reaches the final output approval boundary"
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
