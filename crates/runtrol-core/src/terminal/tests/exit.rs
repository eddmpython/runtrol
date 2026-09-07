//! Process exit and output completion are separate facts.

use super::*;

struct HeldReader {
    inner: Box<dyn TerminalRead>,
    ready: Option<oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
}

impl Read for HeldReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        if count > 0
            && let Some(ready) = self.ready.take()
        {
            ready
                .send(())
                .map_err(|()| std::io::Error::other("the output barrier receiver closed"))?;
            self.release
                .recv_timeout(Duration::from_secs(10))
                .map_err(std::io::Error::other)?;
        }
        Ok(count)
    }
}

impl TerminalRead for HeldReader {}

struct FailedReader;

impl Read for FailedReader {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected terminal read failure",
        ))
    }
}

impl TerminalRead for FailedReader {}

#[test]
fn an_interrupted_read_retries_and_a_later_fault_follows_its_accepted_partial_chunk() {
    struct InterruptedReader(std::collections::VecDeque<std::io::Result<Vec<u8>>>);
    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let bytes = self
                .0
                .pop_front()
                .ok_or_else(|| std::io::Error::other("the fixture has no next read"))??;
            buffer
                .get_mut(..bytes.len())
                .ok_or_else(|| std::io::Error::other("the fixture output exceeds its buffer"))?
                .copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }
    impl TerminalRead for InterruptedReader {
        fn available(&mut self) -> usize {
            usize::from(!self.0.is_empty())
        }
    }
    let reader = InterruptedReader(std::collections::VecDeque::from([
        Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
        Ok(b"last-frame".to_vec()),
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected read fault",
        )),
    ]));
    let (sent, mut received) = mpsc::channel(RING_CHUNKS);
    read_terminal(Box::new(reader), &sent);
    drop(sent);
    assert_eq!(
        received
            .blocking_recv()
            .expect("the accepted bytes are published")
            .expect("first receipt is output"),
        b"last-frame".as_slice()
    );
    assert_eq!(
        received
            .blocking_recv()
            .expect("the fault follows the bytes")
            .expect_err("second receipt is failure")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(received.blocking_recv().is_none());
}

#[tokio::test]
async fn an_output_read_failure_ends_the_exact_owner_instead_of_silently_losing_its_screen() {
    let program = runtrol_childproc::resolve("cmd").expect("the platform shell resolves");
    let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("UTF-8 temporary root"))
        .expect("the temporary root canonicalizes");
    let arguments = vec![
        "/q".to_owned(),
        "/d".to_owned(),
        "/c".to_owned(),
        "set /p retained=".to_owned(),
    ];
    let size = PtySize { cols: 80, rows: 24 };
    let child = PtyChild::spawn(PtySpawn {
        containment: runtrol_childproc::PtyContainment::Local,
        program: &program,
        arguments: &arguments,
        cwd: &cwd,
        env: &[],
        env_unset: &[],
        size,
    })
    .expect("the owned shell waits for input");
    let identity =
        runtrol_childproc::process_identity(child.pid()).expect("the owned child identity");
    eprintln!("owned read-failure fixture: {identity:?}");
    let writer = child.writer().expect("the terminal writes");
    let terminal = Terminal::host_with(Child::Pty(child), Box::new(FailedReader), writer, size)
        .expect("the shell is hosted through the failed reader");
    let mut exited = terminal.shared.exited.subscribe();
    let reported = tokio::time::timeout(Duration::from_secs(2), async {
        while exited.borrow().exit_code.is_none() {
            exited
                .changed()
                .await
                .expect("the host owns its exit publisher");
        }
    })
    .await
    .is_ok();
    // Reclaim the exact fixture before asserting the old implementation's missing failure transition.
    if !reported {
        terminal
            .shared
            .child
            .kill()
            .expect("the owned fixture is stopped");
    }
    tokio::time::timeout(Duration::from_secs(5), terminal.shared.child.wait())
        .await
        .expect("the exact fixture cleanup completes")
        .expect("the process and Job ended");
    assert!(!runtrol_childproc::matches_process_start(
        identity.pid(),
        identity.started()
    ));
    assert!(
        reported,
        "an unreadable output lane left its owner running without a usable screen"
    );
    assert_eq!(
        exited.borrow().failure,
        Some(TerminalFailure::OutputReadFailed)
    );
}

#[tokio::test]
async fn process_exit_waits_for_the_final_output_publication() {
    let program = runtrol_childproc::resolve("cmd").expect("the platform shell resolves");
    let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("UTF-8 temporary root"))
        .expect("the temporary root canonicalizes");
    let arguments = vec![
        "/q".to_owned(),
        "/d".to_owned(),
        "/c".to_owned(),
        "echo last-frame".to_owned(),
    ];
    let size = PtySize { cols: 80, rows: 24 };
    let (child, resume) = PtyChild::prepare(PtySpawn {
        containment: runtrol_childproc::PtyContainment::Local,
        program: &program,
        arguments: &arguments,
        cwd: &cwd,
        env: &[],
        env_unset: &[],
        size,
    })
    .expect("the exact owned shell is suspended before fixture setup");
    let identity =
        runtrol_childproc::process_identity(child.pid()).expect("the suspended child identity");
    eprintln!("owned exit fixture: {identity:?}");
    let (ready, waiting) = oneshot::channel();
    let (release, held) = std::sync::mpsc::sync_channel(1);
    let reader = Box::new(HeldReader {
        inner: child.reader().expect("the terminal reads"),
        ready: Some(ready),
        release: held,
    });
    let writer = child.writer().expect("the terminal writes");
    let terminal = Terminal::host_with(Child::Pty(child), reader, writer, size)
        .expect("the shell is hosted before execution");
    let mut viewer = terminal.attach().await;
    resume
        .resume()
        .expect("execute the one owned primary thread");
    tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the shell produced output")
        .expect("the reader reached the barrier");
    tokio::time::timeout(Duration::from_secs(5), terminal.shared.child.wait())
        .await
        .expect("the echo process exits independently from the held output reader")
        .expect("the exact child and its descendants ended");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let premature = viewer.exited.borrow().exit_code.is_some();
    release
        .send(())
        .expect("release the exact owned reader before assertions");
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !output
            .windows(b"last-frame".len())
            .any(|window| window == b"last-frame")
        {
            output.extend_from_slice(
                &viewer
                    .live
                    .recv()
                    .await
                    .expect("the final output remains in the ring")
                    .bytes,
            );
        }
        while viewer.exited.borrow().exit_code.is_none() {
            viewer
                .exited
                .changed()
                .await
                .expect("the terminal owns its exit publisher");
        }
    })
    .await
    .expect("the released final frame and the exit both arrive");
    assert!(
        !premature,
        "the exit was published while a final frame was still held by the reader"
    );
    assert!(!runtrol_childproc::matches_process_start(
        identity.pid(),
        identity.started()
    ));
}
