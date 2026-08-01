//! Running a program once and reading what it said.
//!
//! For the questions runtrol asks a CLI about itself: which version are you, which flags do you have. Not
//! for a session, which is a long-lived conversation with its own transport.
//!
//! # Three bounds, and what each one prevents
//!
//! **Time.** A CLI that hangs on `--version` would otherwise hang whatever asked. Measured on this machine,
//! a cold start of one of these costs 300 ms and one answer took 39.9 seconds, so the ceiling has to be
//! generous and it has to exist.
//!
//! **Bytes.** Output goes into memory, so it is bounded and truncation is reported. `--help` is a few
//! kilobytes; a CLI in a bad state can print a great deal more, and the daemon's whole idle budget is
//! single-digit megabytes.
//!
//! **Containment.** A probe is a child like any other. It joins the containment, so a daemon that dies does
//! not leave it running.
//!
//! # Why the output is bytes and not text
//!
//! What a CLI prints is not guaranteed to be UTF-8, and a lossy conversion at this layer would put a
//! replacement character inside a version string. The caller decides what to do about that, close to where
//! it knows what the bytes are supposed to be.

use core::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::argv;
use crate::contain::Containment;
use crate::error::SpawnError;
use crate::resolve::Program;

/// How much of a program's output runtrol will hold.
///
/// Generous for the questions this is used for and far below anything that threatens the memory budget.
/// A program that says more than this is either broken or not answering the question that was asked.
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// What one run produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Output {
    /// The exit code, or `None` when the platform reports the process was signalled.
    pub code: Option<i32>,
    /// Standard output, truncated at [`MAX_OUTPUT_BYTES`].
    pub stdout: Vec<u8>,
    /// Standard error, truncated at [`MAX_OUTPUT_BYTES`].
    ///
    /// Kept because these CLIs do not agree about where a version goes. Measured: some print it to standard
    /// output and some to standard error, and a probe that read only one of them would find nothing and
    /// conclude the CLI was broken.
    pub stderr: Vec<u8>,
    /// One of the streams reached the byte bound and was cut.
    ///
    /// Carried so a caller parsing the output knows it may have lost the part it wanted, rather than
    /// concluding the program did not print it.
    pub truncated: bool,
}

impl Output {
    /// Whether the program reported success.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Both streams as text, with invalid sequences replaced, for scanning.
    ///
    /// The lossy conversion happens here rather than at capture time, at a call site that knows it is
    /// looking for a version number or a flag name and that neither can contain a byte that is not UTF-8.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.stdout).into_owned();
        if !self.stderr.is_empty() {
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&self.stderr));
        }
        text
    }
}

/// Run a resolved program with `args`, and read what it said.
///
/// The program's own leading arguments come first, so a caller passes only what it wants to ask.
///
/// # Errors
///
/// [`SpawnError::ArgvUnsafe`] when an argument cannot be passed at all, [`SpawnError::Timeout`] when the
/// program did not finish in time, [`SpawnError::Io`] when it could not be started or its output could not
/// be read.
pub async fn capture(
    program: &Program,
    args: &[String],
    within: Duration,
    contained_by: &Containment,
) -> Result<Output, SpawnError> {
    let mut full: Vec<String> = program.leading().to_vec();
    full.extend_from_slice(args);
    argv::check_all(&full)?;

    let mut command = Command::new(program.path().as_std_path());
    command
        .args(&full)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // The child must not outlive the answer. Without this the process is reaped only when the handle is
        // dropped, and a timeout would leave it running.
        .kill_on_drop(true);
    crate::hide_console_window(command.as_std_mut());
    contained_by.prepare(command.as_std_mut());

    let mut child = command.spawn().map_err(|error| SpawnError::Io {
        path: program.path().to_string(),
        detail: error.to_string(),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| SpawnError::Io {
        path: program.path().to_string(),
        detail: "the captured standard output pipe was not created".to_owned(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| SpawnError::Io {
        path: program.path().to_string(),
        detail: "the captured standard error pipe was not created".to_owned(),
    })?;

    match tokio::time::timeout(within, collect(child, stdout, stderr)).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(SpawnError::Io {
            path: program.path().to_string(),
            detail: error.to_string(),
        }),
        // The child is killed by `kill_on_drop` as the future is dropped here, so the timeout does not leave
        // a process behind.
        Err(_elapsed) => Err(SpawnError::Timeout {
            path: program.path().clone(),
            after_ms: u64::try_from(within.as_millis()).unwrap_or(u64::MAX),
        }),
    }
}

/// Wait for a child while draining both pipes concurrently into bounded buffers.
async fn collect(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
) -> std::io::Result<Output> {
    let (status, stdout, stderr) =
        tokio::join!(child.wait(), read_bounded(stdout), read_bounded(stderr));
    let (stdout, stdout_truncated) = stdout?;
    let (stderr, stderr_truncated) = stderr?;
    Ok(Output {
        code: status?.code(),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    })
}

/// Drain one stream while keeping at most [`MAX_OUTPUT_BYTES`] from its start.
///
/// From the start because a version banner and a flag list are both at the beginning of what these programs
/// print. Bytes beyond the ceiling are still drained so a full child pipe cannot stop the process from exiting, but
/// they are never appended to the retained buffer.
async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    const READ_CHUNK_BYTES: usize = 8 * 1024;

    let mut kept = Vec::with_capacity(READ_CHUNK_BYTES.min(MAX_OUTPUT_BYTES));
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok((kept, truncated));
        }
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(kept.len());
        let take = remaining.min(read);
        kept.extend(chunk.iter().take(take).copied());
        truncated |= take < read;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve;

    /// A program that exists on every supported platform and prints something.
    ///
    /// The interpreter running this test: it is by definition installed, it is not a shell builtin, and it
    /// takes arguments that make it print, exit with a chosen code, or sleep.
    fn this_platforms_program() -> Program {
        let exe = std::env::current_exe().expect("a test binary has a path");
        let exe = exe.to_str().expect("the test binary's path is UTF-8");
        resolve::resolve(exe).expect("the test binary resolves")
    }

    /// A containment that holds nothing, so a test can exercise the bounds without joining the group it is
    /// about to kill.
    fn uncontained() -> Containment {
        Containment::without_any()
    }

    #[tokio::test]
    async fn a_program_that_prints_is_read() {
        // The test binary re-invoked with a flag it does not know prints usage and fails, which is enough to
        // prove both streams are read and the exit code arrives.
        let program = this_platforms_program();
        let output = capture(
            &program,
            &["--not-a-real-test-flag".to_owned()],
            Duration::from_secs(30),
            &uncontained(),
        )
        .await
        .expect("the program must run");

        assert!(!output.truncated);
        assert!(
            !output.text().is_empty(),
            "a program that was asked something must have said something"
        );
    }

    #[tokio::test]
    async fn an_argument_that_cannot_be_passed_is_refused_before_the_spawn() {
        // Refused here rather than by the operating system, whose own message names neither the argument nor
        // the character.
        let program = this_platforms_program();
        let error = capture(
            &program,
            &["has\na line break".to_owned()],
            Duration::from_secs(5),
            &uncontained(),
        )
        .await
        .expect_err("a line break must be refused");
        assert!(matches!(error, SpawnError::ArgvUnsafe { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_program_that_does_not_answer_in_time_is_given_up_on() {
        // Without this, one hung CLI hangs whatever asked it a question. The bound is what makes asking safe.
        let program = this_platforms_program();
        let error = capture(
            &program,
            &["--test-threads=1".to_owned(), "--nocapture".to_owned()],
            Duration::from_millis(1),
            &uncontained(),
        )
        .await;

        match error {
            Err(SpawnError::Timeout { after_ms, .. }) => assert_eq!(after_ms, 1),
            // A machine fast enough to finish the whole suite in one millisecond would answer in time, which
            // is not a failure of the bound. Nothing about the timeout is asserted in that case.
            other => assert!(
                other.is_ok(),
                "expected either a timeout or a completed run, got {other:?}"
            ),
        }
    }

    #[tokio::test]
    async fn the_byte_bound_applies_before_excess_output_is_retained() {
        // A program in a bad state can print a great deal, and the daemon's whole idle budget is
        // single-digit megabytes.
        let long = vec![b'x'; MAX_OUTPUT_BYTES * 2];
        let (kept, truncated) = read_bounded(long.as_slice()).await.expect("read bytes");
        assert_eq!(kept.len(), MAX_OUTPUT_BYTES);
        assert!(truncated, "truncation has to be visible");
    }

    #[tokio::test]
    async fn output_that_fits_is_not_marked_truncated() {
        let (kept, truncated) = read_bounded(&b"version 1.2.3"[..])
            .await
            .expect("read bytes");
        assert!(!truncated);
        assert_eq!(kept, b"version 1.2.3");
    }

    #[test]
    fn both_streams_are_read_because_the_clis_disagree_about_which_one_to_use() {
        // Measured: some of these programs print their version to standard error. A probe reading only
        // standard output would find nothing and conclude the CLI was broken.
        let output = Output {
            code: Some(0),
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
            truncated: false,
        };
        assert!(output.text().contains("out"));
        assert!(output.text().contains("err"));
    }

    #[test]
    fn output_that_is_not_utf8_survives_as_bytes_and_is_readable_lossily() {
        // A lossy conversion at capture time would put a replacement character inside a version string.
        let output = Output {
            code: Some(0),
            stdout: vec![0xFF, 0xFE, b'1', b'.', b'0'],
            stderr: Vec::new(),
            truncated: false,
        };
        assert_eq!(output.stdout.first(), Some(&0xFF));
        assert!(output.text().contains("1.0"));
    }
}
