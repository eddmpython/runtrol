//! The owner-local bridge drains its exact final output before reporting completion.

use super::*;
use crate::runtime_terminal::view_control_tests::Fixture;

#[tokio::test]
async fn terminal_completion_drains_the_local_bridge_and_keeps_structural_failure() {
    for lagged in [false, true] {
        local_completion(lagged).await;
    }
}

async fn local_completion(lagged: bool) {
    let fixture = Fixture::new().await;
    let terminal = fixture.view.hosted.terminal.clone();
    let mut attachment = terminal.attach().await;
    let mut observer = terminal.attach().await;
    terminal
        .feed(b"local final frame".to_vec())
        .expect("feed the final bytes");
    observer
        .live
        .recv()
        .await
        .expect("raw output reaches the bounded ring");
    terminal.end_feed(Some(0)).expect("end the exact feed");
    while observer.exited.borrow().exit_code.is_none() {
        observer.exited.changed().await.expect("Core completes");
    }
    let (completion, exit_state) = watch::channel(runtrol_core::terminal::TerminalCompletion {
        exit_code: Some(0),
        failure: Some(runtrol_core::terminal::TerminalFailure::OutputReadFailed),
    });
    attachment.exited = exit_state;
    let (lag, receiver) = tokio::sync::broadcast::channel(1);
    if lagged {
        attachment.live = receiver;
        for sequence in 1..=2 {
            lag.send(runtrol_core::terminal::OutputChunk {
                sequence,
                bytes: b"lag fixture".to_vec().into(),
            })
            .expect("overflow one bounded viewer");
        }
    }
    let address = fixture
        .composed
        .home
        .paths()
        .endpoint()
        .address()
        .to_owned();
    let mut listener = runtrol_ipc::transport::Listener::bind(&address)
        .await
        .expect("bind the owned fixture endpoint");
    let (server, client) =
        tokio::join!(listener.accept(), runtrol_ipc::transport::connect(&address));
    let mut server = SurfaceConnection::Local(server.expect("accept local viewer"));
    let mut client = client.expect("connect local viewer");
    tokio::time::timeout(
        Duration::from_secs(2),
        relay_local_broker(
            &mut server,
            &fixture.composed,
            fixture.view.hosted.id,
            terminal.clone(),
            attachment,
            None,
        ),
    )
    .await
    .expect("the final local frames drain");
    drop(server);
    let mut frames = Vec::new();
    while let Some(frame) = client.recv().await.expect("receive a local frame") {
        frames.push(serde_json::from_slice::<Response>(&frame).expect("decode local response"));
    }
    drop(client);
    drop(listener);
    drop(observer);
    drop(terminal);
    drop(completion);
    drop(lag);
    fixture.close().await;
    assert!(frames.iter().any(|frame| matches!(frame, Response::TerminalOutput { bytes } if bytes.as_ref().windows(b"local final frame".len()).any(|part| part == b"local final frame"))));
    assert_eq!(
        frames
            .iter()
            .any(|frame| matches!(frame, Response::TerminalLagged {})),
        lagged
    );
    assert!(
        matches!(frames.last(), Some(Response::TerminalExited { code: 0, failure: Some(reason) }) if reason.as_ref() == "outputReadFailed")
    );
}
