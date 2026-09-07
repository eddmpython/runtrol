//! Exact ordered input, including byte sequences indistinguishable from terminal replies.

use super::*;

struct InputRecorder(mpsc::Sender<Vec<u8>>);

impl Write for InputRecorder {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .blocking_send(bytes.to_vec())
            .map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn host_recorder() -> (Terminal, mpsc::Receiver<Vec<u8>>) {
    let child = Child::Fed(FedChild::new(0));
    let reader = child.reader().expect("one synthetic reader");
    let (sent, received) = mpsc::channel(16);
    let terminal = Terminal::host_with(
        child,
        reader,
        Box::new(InputRecorder(sent)),
        PtySize { cols: 80, rows: 24 },
    )
    .expect("host an ordered writer without a provider process");
    (terminal, received)
}

pub(super) async fn finish(terminal: &Terminal) {
    terminal.end_feed(Some(0)).expect("end the synthetic feed");
    let mut exited = terminal.exited();
    while exited.borrow().exit_code.is_none() {
        exited
            .changed()
            .await
            .expect("the exact synthetic host ends");
    }
}

async fn recorded_input(frames: &[&[u8]]) -> Vec<Vec<u8>> {
    let (terminal, mut received) = host_recorder();
    for frame in frames {
        // Each write is another view of the same host. No unfinished input may migrate between them.
        terminal
            .clone()
            .input(frame)
            .await
            .expect("acknowledge the exact input frame");
    }
    finish(&terminal).await;
    let mut writes = Vec::new();
    // Every write was acknowledged already, so no writer can publish another frame after this drain.
    while let Ok(bytes) = received.try_recv() {
        writes.push(bytes);
    }
    writes
}

#[tokio::test]
async fn another_view_cannot_complete_a_departed_viewers_partial_reply() {
    let frames = [b"\x1b[?6".as_slice(), b"2;22csecond"];
    assert_eq!(recorded_input(&frames).await, frames);
}

#[tokio::test]
async fn reply_shaped_keys_paste_ime_and_escape_reach_the_writer_unchanged() {
    let bytes = "\x1b[1;2R\x1b[97u\x1b[13;2u\x1b[9n\x1b[123y\x1b[5c\x1b[0;4R\x1b[200~literal \x1b[12;40R\x1b[?0u\x1b[201~한글\r\n\x03\x1b".as_bytes();
    assert_eq!(recorded_input(&[bytes]).await, [bytes]);
}

#[tokio::test]
async fn interleaved_paste_and_partial_key_frames_are_never_retained_or_combined() {
    let frames = [
        b"\x1b[200~literal".as_slice(),
        b"other viewer",
        b" \x1b[12;40R\x1b[201~",
        b"\x1b[",
        b"97u",
        b"\x1b",
        b"x",
    ];
    assert_eq!(recorded_input(&frames).await, frames);
}

#[tokio::test]
async fn the_host_answers_once_with_zero_or_two_views_while_each_wire_stays_raw() {
    for count in [0, 2] {
        let (terminal, mut written) = host_recorder();
        let mut viewers = Vec::new();
        for _ in 0..count {
            viewers.push(terminal.attach().await);
        }
        let raw = b"abc\x1b[6n\x1b[5n\x1b[?u";
        terminal.shared.take_output(Bytes::from_static(raw)).await;
        let answer = tokio::time::timeout(Duration::from_secs(2), written.recv()).await;
        for viewer in &mut viewers {
            let frame = viewer.live.recv().await.expect("the exact raw wire frame");
            assert_eq!(frame.bytes.as_ref(), raw);
            let mut presentation = runtrol_terminal_protocol::QueryFilter::default();
            assert_eq!(presentation.filter(&frame.bytes), b"abc\x1b\\\x1b\\\x1b\\");
        }
        drop(terminal.attach().await);
        finish(&terminal).await;
        assert_eq!(
            answer
                .expect("the host answers without a viewer")
                .expect("one ordered answer"),
            b"\x1b[1;4R\x1b[0n\x1b[?0u"
        );
        assert!(
            written.try_recv().is_err(),
            "attaching viewers never adds another answer"
        );
    }
}
