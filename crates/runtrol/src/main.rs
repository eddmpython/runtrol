//! The runtrol executable. It picks a personality from argv and does nothing else.
//!
//! Logic does not go here. This file necessarily links every crate (one executable, three personalities), so it is the
//! one place where every architectural boundary is invisible. Keeping it empty of decisions is what keeps those
//! boundaries meaningful: the command surface is a crate that cannot see storage, the daemon is a crate a test can
//! link, and this is neither.
//!
//! # Three personalities
//!
//! - **A daemon**, asked for by name, which serves until it is stopped.
//! - **A command**, which is everything else somebody types.
//! - **A daemon started by a command**, which is the first one again, launched by the second when nothing is
//!   listening. It is not a fourth thing to build: it is why the first is a subcommand rather than a separate program.
//!
//! # Why one executable
//!
//! Installing runtrol installs one file. That is what makes "the same method everywhere" possible: no runtime to
//! install first, no second program to keep in step, and an update that replaces one file rather than a set. It is
//! also what lets a command start a daemon at all, because the program it needs is the one already running.

use std::process::ExitCode;

/// Which personality this invocation is.
enum Personality {
    /// Serve, until stopped.
    Daemon,
    /// Open the window.
    Window,
    /// Ask the daemon something and print the answer.
    Command(Vec<String>),
    /// Say what the words could have been.
    Usage(String),
}

/// The word that opens the window.
///
/// Spelled here beside the daemon's word rather than inferred from how the program was invoked, for the same
/// reason: a renamed file behaving differently is a surprise nobody asked for.
const WINDOW_ARGUMENT: &str = "gui";

fn main() -> ExitCode {
    // Before anything could be started. Whether this program's own handles may travel to what it starts is a property
    // of the process, so it is set once here rather than argued about at each spawn. Measured: without it, a command
    // that starts a daemon hands that daemon a copy of the shell's own pipe, and the shell waits forever with nothing
    // to show for it.
    if let Err(error) = runtrol_childproc::keep_handles_to_ourselves() {
        report(&format!("runtrol cannot start: {error}"));
        return ExitCode::FAILURE;
    }

    let words: Vec<String> = std::env::args().skip(1).collect();
    let personality = choose(&words);
    if matches!(personality, Personality::Window)
        && let Err(error) = runtrol_childproc::hide_if_private()
    {
        report(&format!(
            "runtrol cannot prepare its desktop window: {error}"
        ));
        return ExitCode::FAILURE;
    }
    match personality {
        Personality::Daemon => run(serving()),
        // No runtime is built for this one. The window's own toolkit owns the main thread and brings a runtime
        // with it, and wrapping it in a second would be two schedulers for one process.
        Personality::Window => showing(),
        Personality::Command(words) => run(commanding(&words)),
        Personality::Usage(message) => {
            report(&message);
            ExitCode::FAILURE
        }
    }
}

/// Read argv, and nothing else.
fn choose(words: &[String]) -> Personality {
    match words.first().map(String::as_str) {
        None => Personality::Usage(
            "runtrol <command>. try: gui, list, start, resume, say, answer, stop, watch, close, panic"
                .to_owned(),
        ),
        // Spelled as a subcommand rather than inferred from how the program was invoked. Inferring it from the
        // executable's own name would mean a renamed file behaving differently, which is a surprise nobody asked for.
        Some(word) if word == runtrol_cli::DAEMON_ARGUMENT => Personality::Daemon,
        Some(word) if word == WINDOW_ARGUMENT => Personality::Window,
        Some(_) => Personality::Command(words.to_vec()),
    }
}

/// Run one personality on a runtime of its own.
///
/// Built here rather than by an attribute on `main`, because the two personalities want different runtimes and an
/// attribute can only say one thing. A command is one connection and some waiting; a daemon supervises processes.
fn run<Work>(work: Work) -> ExitCode
where
    Work: FnOnce(&tokio::runtime::Runtime) -> ExitCode,
{
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => work(&runtime),
        Err(error) => {
            report(&format!("runtrol cannot start: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Be the daemon.
fn serving() -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    |runtime| {
        // Composing establishes containment before any child could exist, so it happens before anything is served.
        let composed = match runtrol_daemon::Composed::assemble(None, runtrol_drivers::builtin()) {
            Ok(composed) => composed,
            Err(error) => {
                report(&format!("runtrol cannot start a daemon: {error}"));
                return ExitCode::FAILURE;
            }
        };
        let address = composed.home.paths().endpoint().address().to_owned();

        // The endpoint and the serving are both on the runtime, because an endpoint has to be created there.
        let served = runtime.block_on(async move {
            let listener = runtrol_ipc::transport::Listener::bind(&address).await?;
            runtrol_daemon::serve(composed, listener).await
        });

        match served {
            Ok(()) => ExitCode::SUCCESS,
            // The ordinary reason for failing to listen is that another daemon is already serving this home, which is
            // not a failure of anything: the command that started this one reaches that one instead.
            Err(error) => {
                report(&format!("runtrol stopped serving: {error}"));
                ExitCode::FAILURE
            }
        }
    }
}

/// Be the window.
///
/// Where the daemon listens and which program starts one are decided here and handed over, the same two values
/// the command surface is given and for the same reason: a library that worked out "whatever process this is"
/// would run the test runner inside a test.
fn showing() -> ExitCode {
    let address = match runtrol_daemon::endpoint(None) {
        Ok(address) => address,
        Err(error) => {
            report(&format!(
                "cannot tell where runtrol keeps its files: {error}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            report(&format!("cannot tell where runtrol itself is: {error}"));
            return ExitCode::FAILURE;
        }
    };

    match runtrol_gui::run(runtrol_gui::Reaching {
        address,
        runtrol: executable,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&format!("runtrol could not open its window: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Be a command.
fn commanding(words: &[String]) -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    move |runtime| {
        let here = match std::env::current_dir() {
            Ok(here) => here.to_string_lossy().into_owned(),
            Err(error) => {
                report(&format!("cannot tell which directory this is: {error}"));
                return ExitCode::FAILURE;
            }
        };

        let request = match runtrol_cli::understand(words, &here) {
            Ok(request) => request,
            Err(misunderstood) => {
                report(&misunderstood.to_string());
                return ExitCode::FAILURE;
            }
        };

        // Where a daemon for this home listens. Asked for rather than worked out, so the two ends cannot derive it
        // differently.
        let address = match runtrol_daemon::endpoint(None) {
            Ok(address) => address,
            Err(error) => {
                report(&format!(
                    "cannot tell where runtrol keeps its files: {error}"
                ));
                return ExitCode::FAILURE;
            }
        };

        // The program a daemon is started from, when there is none. This one, named rather than inferred inside the
        // command surface: a library that ran "whatever process this is" would run the test runner inside a test.
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                report(&format!("cannot tell where runtrol itself is: {error}"));
                return ExitCode::FAILURE;
            }
        };

        match runtime.block_on(runtrol_cli::ask(&address, &executable, request, say)) {
            Ok(runtrol_cli::Outcome::Carried) => ExitCode::SUCCESS,
            // The daemon answered and the answer was no. Already written for the operator by the surface, so
            // nothing more is said here; what changes is the status, because a command that did not do what it
            // was asked must not report that it did to whatever ran it.
            Ok(runtrol_cli::Outcome::Refused) => ExitCode::FAILURE,
            Err(failure) => {
                report(&failure.to_string());
                ExitCode::FAILURE
            }
        }
    }
}

/// One line of an answer.
///
/// Answers go to standard output so that a listing can be piped into something else, and everything runtrol says about
/// itself goes to standard error so that it never lands in the middle of one.
#[expect(
    clippy::print_stdout,
    reason = "an answer to a command is what this executable is for"
)]
fn say(line: &str) {
    println!("{line}");
}

/// Say something to whoever ran this.
#[expect(
    clippy::print_stderr,
    reason = "an executable has to be able to tell the person who ran it what went wrong"
)]
fn report(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_daemon_is_asked_for_by_name_and_never_inferred() {
        // Inferring it from the executable's own name would mean a renamed file behaving differently, which is a
        // surprise nobody asked for and a way to start a daemon by accident.
        assert!(matches!(choose(&typed("daemon")), Personality::Daemon));
        assert!(matches!(choose(&typed("list")), Personality::Command(_)));
        assert!(matches!(choose(&typed("Daemon")), Personality::Command(_)));
    }

    #[test]
    fn the_word_this_reads_is_the_word_the_command_surface_types() {
        // A command starts a daemon by running this executable with one argument. Two spellings of that word would be
        // a daemon started that answers nothing, and a command waiting the full ten seconds to say so.
        assert!(matches!(
            choose(&typed(runtrol_cli::DAEMON_ARGUMENT)),
            Personality::Daemon
        ));
    }

    #[test]
    fn running_it_with_nothing_says_what_it_could_have_been() {
        match choose(&[]) {
            Personality::Usage(message) => {
                assert!(message.contains("list"), "{message}");
                assert!(message.contains("panic"), "{message}");
            }
            _ => panic!("expected usage"),
        }
    }

    #[test]
    fn a_command_reaches_the_surface_with_every_word_intact() {
        // This file passes words along and reads none of them. A word lost here is a word the surface never sees.
        let words = typed("say abc write the thing");
        match choose(&words) {
            Personality::Command(passed) => assert_eq!(passed, words),
            _ => panic!("expected a command"),
        }
    }
}
