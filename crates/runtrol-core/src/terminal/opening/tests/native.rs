use super::{FailedHost, open, open_with_cleanup};
use crate::terminal::{Child, PtySize, SpawnError, Terminal, TerminalError, TerminalLaunch};
use runtrol_provider::{AbsPath, ProcessIdentity};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FIXTURE_MARKER: &str = "RUNTROL_TERMINAL_OPEN_FAILURE_FIXTURE";
const FIXTURE_TEST: &str = "terminal::opening::tests::native_open_failure_fixture";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "runtrol-terminal-opening-{}-{now}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn marker(&self) -> PathBuf {
        self.0.join("native-ready")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // This exact directory was created above beneath the configured task TEMP, never a project.
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
#[ignore = "private bounded native process fixture, invoked only by the opening tests"]
fn native_open_failure_fixture() {
    let Some(marker) = std::env::var_os(FIXTURE_MARKER) else {
        return;
    };
    std::fs::write(marker, std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

fn launch<'a>(
    scratch: &Scratch,
    program: &'a runtrol_childproc::Program,
    cwd: &'a AbsPath,
) -> TerminalLaunch<'a> {
    TerminalLaunch {
        program,
        arguments: vec![
            "--exact".to_owned(),
            FIXTURE_TEST.to_owned(),
            "--ignored".to_owned(),
            "--nocapture".to_owned(),
        ],
        cwd,
        env: vec![(
            FIXTURE_MARKER.to_owned(),
            scratch.marker().to_str().unwrap().to_owned(),
        )],
        env_unset: Vec::new(),
        size: PtySize { cols: 80, rows: 24 },
    }
}

fn fail_reader_start(
    child: Child,
    size: PtySize,
    marker: &Path,
    identity: &mut Option<ProcessIdentity>,
) -> Result<Terminal, FailedHost> {
    *identity = runtrol_childproc::process_identity(child.pid());
    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        match std::fs::read_to_string(marker) {
            Ok(contents) if contents == child.pid().to_string() => break true,
            Ok(_) => {} // The fixture may still be writing the readiness file.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(FailedHost {
                    child,
                    cause: Box::new(TerminalError::Runtime(format!(
                        "reading native readiness: {error}"
                    ))),
                });
            }
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    if !ready || identity.is_none() {
        return Err(FailedHost {
            child,
            cause: Box::new(TerminalError::Runtime(
                "the owned native fixture did not reach readiness".to_owned(),
            )),
        });
    }
    eprintln!(
        "owned native fixture: identity={identity:?}, executable={:?}",
        std::env::current_exe()
    );
    let reader = match child.reader() {
        Ok(reader) => reader,
        Err(error) => {
            return Err(FailedHost {
                child,
                cause: Box::new(error.into()),
            });
        }
    };
    let writer = match child.writer() {
        Ok(writer) => writer,
        Err(error) => {
            return Err(FailedHost {
                child,
                cause: Box::new(error.into()),
            });
        }
    };
    Terminal::host_with_reader(child, reader, writer, size, |_reader, _chunks| {
        Err(std::io::Error::other(
            "injected reader thread start failure after native readiness",
        ))
    })
}

fn live(identity: ProcessIdentity) -> bool {
    runtrol_childproc::matches_process_start(identity.pid(), identity.started())
}

#[tokio::test]
async fn a_failed_reader_start_never_returns_an_unowned_live_root() {
    let program =
        runtrol_childproc::resolve(std::env::current_exe().unwrap().to_str().unwrap()).unwrap();
    for _ in 0..8 {
        let scratch = Scratch::new();
        let cwd = AbsPath::canonicalize(scratch.0.to_str().unwrap()).unwrap();
        let mut identity = None;
        let error = open(&launch(&scratch, &program, &cwd), |child, size| {
            fail_reader_start(child, size, &scratch.marker(), &mut identity)
        })
        .unwrap_err();
        let identity =
            identity.expect("the real native root was identified before failure injection");
        let (returned_unowned_live, cause) = match error {
            TerminalError::CleanupIncomplete {
                terminal, cause, ..
            } => {
                assert_eq!(terminal.pid(), identity.pid());
                let mut exited = terminal.exited();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while exited.borrow().is_none() {
                        exited.changed().await.unwrap();
                    }
                })
                .await
                .unwrap();
                (false, *cause)
            }
            cause => (live(identity), cause),
        };
        // Keep cleanup finite even when running this assertion against the old asynchronous Drop path.
        let deadline = Instant::now() + Duration::from_secs(5);
        while live(identity) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(
            !live(identity),
            "owned native root {} survived cleanup",
            identity.pid()
        );
        assert!(
            !returned_unowned_live,
            "ordinary Err released native root {} before its exit",
            identity.pid()
        );
        assert!(
            matches!(cause, TerminalError::Runtime(ref detail) if detail.contains("injected reader thread"))
        );
    }
}

#[tokio::test]
async fn a_failed_exit_confirmation_retains_the_real_root_and_rejects_io() {
    use std::future::Future as _;
    let scratch = Scratch::new();
    let program =
        runtrol_childproc::resolve(std::env::current_exe().unwrap().to_str().unwrap()).unwrap();
    let cwd = AbsPath::canonicalize(scratch.0.to_str().unwrap()).unwrap();
    let mut identity = None;
    let error = open_with_cleanup(
        &launch(&scratch, &program, &cwd),
        |child, size| fail_reader_start(child, size, &scratch.marker(), &mut identity),
        |_child| {
            Err(SpawnError::Pty {
                doing: "confirming terminal process termination",
                detail: "injected bounded exit confirmation failure".to_owned(),
            })
        },
    )
    .unwrap_err();
    let identity = identity.expect("the real native root was identified before failure injection");
    let TerminalError::CleanupIncomplete {
        terminal, cause, ..
    } = error
    else {
        // The old implementation drops the child. Wait for that exact root before reporting the red.
        let deadline = Instant::now() + Duration::from_secs(5);
        while live(identity) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!(
            "an unconfirmed exit must retain native root {} as an owned terminal",
            identity.pid()
        );
    };
    let retained_live = live(identity);
    let same_pid = terminal.pid() == identity.pid();
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let input_refused = matches!(
        std::pin::pin!(terminal.input(b"must not reach the failed host"))
            .as_mut()
            .poll(&mut context),
        std::task::Poll::Ready(Err(TerminalError::Runtime(_)))
    );
    let resize_refused = matches!(
        std::pin::pin!(terminal.resize(PtySize {
            cols: 120,
            rows: 30
        }))
        .as_mut()
        .poll(&mut context),
        std::task::Poll::Ready(Err(TerminalError::Runtime(_)))
    );
    let viewer = terminal.attach().await;
    let checkpoint_unavailable = !viewer.checkpoint_available && viewer.snapshot.is_empty();
    terminal.kill().unwrap();
    // No exit receiver exists while the real process ends. A delayed registry bind can subscribe
    // only after this point and must still observe the exact exit rather than an empty watch value.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !terminal
            .shared
            .finished
            .load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let exited = terminal.exited();
    assert!(
        exited.borrow().is_some(),
        "a late observer must retain the exact native exit"
    );
    assert!(
        !live(identity),
        "the exact exit watcher must report a stopped root"
    );
    assert!(
        retained_live && same_pid,
        "the failed native process must remain explicitly owned"
    );
    assert!(
        input_refused && resize_refused,
        "failed-host input and resize must reject on their first poll"
    );
    assert!(checkpoint_unavailable);
    assert!(cause.to_string().contains("injected reader thread"));
}
