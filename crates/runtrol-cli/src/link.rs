//! Reaching the daemon, and starting one when there is none.
//!
//! # The operator never starts a daemon
//!
//! Typing a command is the whole of what they do. If nothing is listening, one is started and the command carries on,
//! because a product that answers "start the daemon first" has made the person do something a program can do. That is
//! the one rule this module exists for.
//!
//! # What gets started is named by the caller, never inferred
//!
//! [`reach`] is told which program to run. It does not ask the operating system what the running process is, which
//! would make this behave differently depending on what linked it: inside a test that answer is the test runner, and
//! starting a daemon would mean the test runner running itself. Measured, and it forks until the machine is full.
//!
//! Naming it also means there is no value here that came from the operator, so nothing on their command line can
//! become part of a program being launched.
//!
//! # Why starting one is safe to do from a command
//!
//! Only one daemon can serve a home: the endpoint is exclusive, so a second one binding fails immediately, and the
//! database it opens is exclusive too. Two commands racing to start one therefore produce one daemon and one harmless
//! refusal, rather than two daemons over one set of sessions. Nothing here has to coordinate, and nothing here holds a
//! lock file that a machine losing power could leave behind.

use core::time::Duration;
use std::path::Path;

use runtrol_ipc::transport::{Connection, TransportError};

/// The subcommand that makes the runtrol executable a daemon.
///
/// Here rather than in the binary because this is the half that types it. The binary reads it, and a second spelling
/// of one word would be a daemon started that never answers.
pub const DAEMON_ARGUMENT: &str = "daemon";

/// How long to keep trying to reach a daemon that has just been started.
///
/// It has to establish containment, find its home, read the manifests, and bind. Generous against a loaded machine,
/// and short enough that a daemon which failed to start is reported rather than waited on.
const WHILE_STARTING: Duration = Duration::from_secs(10);

/// How long to wait before asking again.
const BETWEEN_TRIES: Duration = Duration::from_millis(25);

/// The daemon could not be reached.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Unreachable {
    /// A daemon could not be started.
    #[error("cannot start a daemon from {program}: {detail}")]
    CannotStart {
        /// What was asked to run.
        program: String,
        /// What the platform said.
        detail: String,
    },

    /// A daemon was started and never began answering.
    #[error("a daemon was started and did not begin answering within {seconds} seconds")]
    NeverAnswered {
        /// How long it was given.
        seconds: u64,
    },

    /// The endpoint is there and something went wrong reaching it.
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Reach a daemon at `address`, starting one from `runtrol` if nothing is listening.
///
/// `runtrol` is the executable to run, which the caller knows and this cannot: see the module notes.
///
/// # Errors
///
/// [`Unreachable`] in every case where no connection could be had. A failure other than "nothing is listening" is
/// never answered by starting a daemon: launching a second one on top of a broken endpoint would replace a
/// diagnosable failure with two.
pub async fn reach(address: &str, runtrol: &Path) -> Result<Connection, Unreachable> {
    trace("cli: connecting");
    match runtrol_ipc::transport::connect(address).await {
        Ok(connection) => {
            trace("cli: connected");
            return Ok(connection);
        }
        Err(error) if error.means_no_daemon() => trace("cli: nothing listening; starting a daemon"),
        Err(error) => return Err(Unreachable::Transport(error)),
    }

    start(runtrol)?;
    trace("cli: daemon spawned; waiting for it to answer");
    wait_for_it(address).await
}

/// One step of reaching the daemon, on stderr, only when `RUNTROL_CLOSE_TRACE=1` asks for it.
///
/// The CI harness is the audience: a command that hangs to its timeout with no output cannot say whether it
/// was connecting, starting a daemon, or waiting on one (measured 2026-08-27: `start` and `close --now` hit
/// 15 s on the Unix hosts while the daemon's own trace stayed silent, which places the stall on this side).
#[expect(
    clippy::print_stderr,
    reason = "the breadcrumb exists to reach the harness's captured stderr, and only when RUNTROL_CLOSE_TRACE=1 asks for it"
)]
pub(crate) fn trace(step: &str) {
    if std::env::var_os("RUNTROL_CLOSE_TRACE").is_some_and(|value| value == "1") {
        static BEGAN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let elapsed = BEGAN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis();
        eprintln!("runtrol +{elapsed}ms {step}");
    }
}

/// Start a daemon, detached from this command.
///
/// It outlives the command that started it, which is the point: the next command finds it already there, and so does
/// the phone. Its own output goes nowhere, because a command surface printing a daemon's startup into the middle of
/// an operator's terminal is noise attached to the wrong process.
///
/// # Why the terminal has to be let go of, and not only the streams
///
/// Giving the daemon no streams is not the same as detaching it. Measured: a daemon started with its streams sent
/// nowhere still held the terminal that started it, so the command returned and the operator's prompt never came
/// back. Nothing was wrong and nothing said so, which is the worst shape a defect can take.
///
/// It also must not be stopped by a keystroke aimed at the command. An operator pressing the interrupt key at a
/// prompt is stopping what they can see; a daemon that went away with it would take every running agent with it.
fn start(runtrol: &Path) -> Result<(), Unreachable> {
    let mut command = std::process::Command::new(runtrol);
    command
        .arg(DAEMON_ARGUMENT)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);

    command.spawn().map_err(|error| Unreachable::CannotStart {
        program: runtrol.display().to_string(),
        detail: error.to_string(),
    })?;
    Ok(())
}

/// Let go of the terminal that started this command.
#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;

    /// Run the background daemon without creating or inheriting a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// And do not receive what somebody types at it.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

/// Let go of the terminal that started this command.
#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;

    // Its own group, so an interrupt aimed at the command running in the foreground does not reach it.
    command.process_group(0);
}

/// Keep asking until the daemon that was just started answers.
async fn wait_for_it(address: &str) -> Result<Connection, Unreachable> {
    let give_up_at = tokio::time::Instant::now() + WHILE_STARTING;
    loop {
        match runtrol_ipc::transport::connect(address).await {
            Ok(connection) => return Ok(connection),

            // Not up yet. The ordinary answer for most of this loop.
            Err(error) if error.means_no_daemon() => {
                if tokio::time::Instant::now() >= give_up_at {
                    return Err(Unreachable::NeverAnswered {
                        seconds: WHILE_STARTING.as_secs(),
                    });
                }
                tokio::time::sleep(BETWEEN_TRIES).await;
            }

            // It is up and something else is wrong. Reported as it is: waiting longer would turn a failure with a
            // reason into one without.
            Err(error) => return Err(Unreachable::Transport(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program that is not there.
    ///
    /// Every test here passes this, so that a regression which starts a daemon when it should not fails by saying it
    /// could not start one, rather than by running whatever the test runner happens to be.
    fn nothing_to_start() -> &'static Path {
        Path::new("this-program-does-not-exist-and-that-is-the-point")
    }

    #[tokio::test]
    async fn a_failure_that_is_not_an_absent_daemon_never_starts_one() {
        // Starting a second daemon on top of an endpoint that is broken for some other reason replaces one
        // diagnosable failure with two, and the second one is the confusing one.
        // Measured on each platform, because guessing at this was wrong once: every unused name in the Windows pipe
        // namespace reads as "nothing is listening", however malformed, so the address that fails differently has to
        // be something outside it.
        let broken = if cfg!(windows) {
            // A directory, which the platform refuses to open with access denied.
            r"C:\Windows"
        } else {
            // A path whose parent is not a directory.
            "/dev/null/runtrol-test-link.sock"
        };

        match reach(broken, nothing_to_start()).await {
            Err(Unreachable::Transport(error)) => assert!(
                !error.means_no_daemon(),
                "this test needs an address that fails for some other reason: {error}"
            ),
            Ok(_) => panic!("a broken address must not produce a connection"),
            Err(other) => panic!("expected the failure to be reported as it was, got {other}"),
        }
    }

    #[tokio::test]
    async fn a_daemon_that_cannot_be_started_says_which_program_was_tried() {
        // The operator's next move is to look at the program named, and a message without it leaves them guessing at
        // what runtrol thought it was running.
        let nowhere = if cfg!(windows) {
            r"\\.\pipe\runtrol-test-link-absent"
        } else {
            "/tmp/runtrol-test-link-absent.sock"
        };

        match reach(nowhere, nothing_to_start()).await {
            Err(Unreachable::CannotStart { program, .. }) => {
                assert!(program.contains("does-not-exist"), "{program}");
            }
            Ok(_) => panic!("nothing is listening there"),
            Err(other) => panic!("expected a refusal naming the program, got {other}"),
        }
    }

    #[test]
    fn the_word_that_makes_a_daemon_is_written_once() {
        // Two spellings of it would be a daemon started that never answers, and a command surface waiting the full
        // ten seconds to say so.
        assert_eq!(DAEMON_ARGUMENT, "daemon");
    }
}
