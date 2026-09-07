//! Exercise the real panic hook in an owned child while a production courier holds unread opaque mail.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use runtrol_courier::wire::{Answer, Hello, Invocation, Request};
use runtrol_courier::{BoundedUtf8, CallEnvelope, Courier, Limits, ManagedSessionId, UnixMillis};
use tokio::io::AsyncWriteExt as _;

const BODY_MARKER: &str = "courier-crash-opaque-body-75629";
const TOKEN_MARKER: &str = "courier-crash-private-token-49163";
const FIXTURE_ENV: &str = "RUNTROL_CRASH_FIXTURE";

#[tokio::test]
async fn panic_report_excludes_queued_courier_bodies_and_tokens() {
    let directory =
        std::env::temp_dir().join(format!("runtrol-courier-crash-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&directory).expect("create owned crash fixture");
    std::fs::write(directory.join("fixture-owner"), "courier crash regression")
        .expect("record owned fixture");
    let output = run_crash(&directory, 1).await;
    let crash =
        std::fs::read(directory.join("daemon-crash.log")).expect("the real hook wrote a report");
    let report = std::str::from_utf8(&crash).expect("the crash report is UTF-8");
    assert!(report.contains("courier crash fixture"));
    assert!(report.contains("Courier") && report.contains("BoundedUtf8"));
    assert!(report.contains("backtrace:"), "the real crash writer ran");
    for bytes in [&crash, &output.stdout, &output.stderr] {
        for marker in [BODY_MARKER, TOKEN_MARKER] {
            assert!(
                !bytes
                    .windows(marker.len())
                    .any(|part| part == marker.as_bytes()),
                "opaque content reached a crash diagnostic"
            );
        }
    }
    std::fs::remove_dir_all(&directory).expect("remove the exact ended crash fixture");
}

#[tokio::test]
async fn real_panic_reports_append_and_rotate_without_exceeding_the_byte_bound() {
    let directory =
        std::env::temp_dir().join(format!("runtrol-crash-rotation-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&directory).expect("create owned rotation fixture");
    std::fs::write(directory.join("fixture-owner"), "crash rotation regression")
        .expect("record owned fixture");
    let report_path = directory.join("daemon-crash.log");
    run_crash(&directory, 2).await;
    run_crash(&directory, 4).await;
    let small = std::fs::read_to_string(&report_path).expect("read actual small reports");
    assert!(small.contains("rotation-first") && small.contains("rotation-after"));
    assert!(small.len() <= super::CRASH_LOG_BOUND_BYTES);
    run_crash(&directory, 5).await;
    let traced = std::fs::read_to_string(&report_path).expect("read the deep real backtrace");
    assert!(traced.contains("rotation-backtrace") && traced.contains("backtrace:"));
    assert!(traced.len() <= super::CRASH_LOG_BOUND_BYTES);
    assert!(traced.ends_with(super::TRUNCATED));
    let huge = run_crash(&directory, 3).await;
    let report = std::fs::read_to_string(&report_path).expect("truncation keeps valid UTF-8");
    assert!(report.contains("rotation-large"));
    assert!(!report.contains("rotation-first") && !report.contains("rotation-after"));
    assert!(report.len() <= super::CRASH_LOG_BOUND_BYTES);
    assert!(report.ends_with(super::TRUNCATED));
    assert!(
        huge.stderr.len() > super::CRASH_LOG_BOUND_BYTES,
        "the inherited foreground hook remains unchanged; the owned file is bounded"
    );
    run_crash(&directory, 4).await;
    let after = std::fs::read_to_string(&report_path).expect("read the post-rotation report");
    assert!(after.contains("rotation-after") && !after.contains("rotation-large"));
    assert!(after.len() <= super::CRASH_LOG_BOUND_BYTES);
    std::fs::remove_dir_all(&directory).expect("remove the exact ended rotation fixture");
}

async fn run_crash(directory: &Path, kind: u8) -> std::process::Output {
    let executable = std::env::current_exe().expect("the test executable has a path");
    let mut command = tokio::process::Command::new(&executable);
    command
        .args([
            "--exact",
            "crash::courier_tests::courier_panic_child",
            "--ignored",
            "--nocapture",
        ])
        .env(FIXTURE_ENV, directory)
        .env("RUST_MIN_STACK", "16777216")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().expect("start the isolated hook process");
    let identity = runtrol_childproc::process_identity(child.id().expect("owned child PID"))
        .expect("record the child before permitting its panic");
    child
        .stdin
        .take()
        .expect("the child is waiting for its owner")
        .write_all(&[kind])
        .await
        .expect("permit the exact owned child to proceed");
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("the bounded panic process exits")
        .expect("collect its exact exit and diagnostics");
    assert_eq!(
        output.status.code(),
        Some(101),
        "the owned child {} must actually panic and exit",
        identity.pid()
    );
    output
}

#[test]
#[ignore = "owned child entry point, invoked by the panic-report regression"]
fn courier_panic_child() {
    use std::io::Read as _;

    let directory =
        PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("the parent names its fixture"));
    assert!(directory.join("fixture-owner").is_file());
    let mut kind = [0];
    std::io::stdin()
        .read_exact(&mut kind)
        .expect("the parent recorded this process");
    let mut courier = Courier::new(Limits::INITIAL);
    let source = ManagedSessionId::now();
    let target = ManagedSessionId::now();
    assert!(courier.session_started(source) && courier.session_started(target));
    let now = UnixMillis(1_800_000_000_000);
    let body = BoundedUtf8::new(
        format!("{BODY_MARKER}: 한글 English \"quoted\" \\ path\nnext line"),
        Limits::INITIAL.body_bytes,
    )
    .expect("the synthetic body is bounded");
    let envelope = CallEnvelope::ask(source, target, body, now.plus(1_000));
    let receipt = courier
        .send(envelope.clone(), now)
        .expect("the real mailbox accepts its ask");
    let refused = courier
        .send(envelope.clone(), now)
        .expect_err("the real duplicate is refused");
    assert_eq!(courier.waiting(target), Some(1));
    assert_eq!(courier.active_calls(), 1);
    assert_eq!(courier.charged_bytes(), envelope.body.len());
    let invocation = Invocation {
        hello: Hello::new(source, TOKEN_MARKER.to_owned()),
        request: Some(Request::Send {
            envelope: envelope.clone(),
        }),
    };
    let answer = Answer::Received {
        envelope: Some(envelope),
    };
    super::record_panics_at(&directory.join("daemon-crash.log"));
    match kind {
        [2] => panic!("rotation-first"),
        [3] => panic!(
            "rotation-large: {}",
            "가".repeat(super::CRASH_LOG_BOUND_BYTES)
        ),
        [4] => panic!("rotation-after"),
        [5] => {
            deep_panic(2048);
        }
        _ => {}
    }
    // Stress every body-bearing diagnostic owner while the actual mailbox still retains its unread ask.
    panic!(
        "courier crash fixture: {courier:?}; {invocation:?}; {answer:?}; {receipt:?}; {refused}"
    );
}

#[inline(never)]
fn deep_panic(depth: usize) -> usize {
    assert!(std::hint::black_box(depth) != 0, "rotation-backtrace");
    // Retain real frames instead of allowing a tail call. Only the isolated test child gets the larger stack.
    std::hint::black_box(deep_panic(depth - 1)) + std::hint::black_box(depth)
}
