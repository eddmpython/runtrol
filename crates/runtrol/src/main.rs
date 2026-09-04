//! The runtrol executable. It picks a personality from argv and does nothing else.
//!
//! Logic does not go here. This file necessarily links the command and daemon crates, so it is the
//! one place where every architectural boundary is invisible. Keeping it empty of decisions is what keeps those
//! boundaries meaningful: the command surface is a crate that cannot see storage, the daemon is a crate a test can
//! link, and this is neither.
//!
//! # Entry points
//!
//! - **A daemon**, asked for by name, which serves until it is stopped.
//! - **A command**, which is everything else somebody types.
//! - **A local endpoint reporter**, used by native surfaces that speak the framed IPC directly.
//! - **A validated Runtime locator reporter**, used by packaged Node.js surfaces on Windows without a shell probe.
//! - **A daemon started by a command**, which is the first one again, launched by the second when nothing is
//!   listening. It is why the first is a subcommand rather than a separate program.
//!
//! # Why one executable
//!
//! Installing runtrol installs one file. That is what makes "the same method everywhere" possible: no runtime to
//! install first, no second program to keep in step, and an update that replaces one file rather than a set. It is
//! also what lets a command start a daemon at all, because the program it needs is the one already running.

use std::io::IsTerminal as _;
use std::process::ExitCode;

/// Which personality this invocation is.
enum Personality {
    /// Serve, until stopped.
    Daemon,
    /// Ensure the daemon is reachable and print its exact local endpoint.
    Endpoint,
    /// Inspect and print the native-client-validated public Runtime locator, preferring one generation digest.
    RuntimeLocator(Option<String>),
    /// Print every daemon generation of this home and whether each still answers.
    Status { json: bool },
    /// Carry one exact provider invocation through the daemon-owned terminal.
    TerminalBridge {
        /// Runtime-discovered provider identity.
        provider: String,
        /// Exact words after the provider command.
        arguments: Vec<String>,
    },
    /// Say which live process holds one file, for the Runtime that spawned this helper, and end.
    WhoHolds(std::path::PathBuf),
    /// Materialize runtime-discovered transparent provider launchers.
    ProviderShims(std::path::PathBuf),
    /// Administer public Runtime integrations from an owner terminal.
    Administration(Vec<String>),
    /// Ask the daemon something and print the answer.
    Command(Vec<String>),
    /// Say what the words could have been.
    Usage(String),
}

/// The word native surfaces use to discover the daemon endpoint from its single owner.
const ENDPOINT_ARGUMENT: &str = "endpoint";

/// The exact bootstrap probe used by packaged Node.js surfaces that already selected this executable.
const RUNTIME_LOCATOR_ARGUMENT: &str = "runtime-locator";

/// The word that lists every daemon generation of this home. Starts nothing.
const STATUS_ARGUMENT: &str = "status";

/// The private entry point used by transparent provider command shims.
const TERMINAL_BRIDGE_ARGUMENT: &str = "bridge";

/// Blocking pipe operations admitted by the daemon at one time on Windows.
///
/// Each hot process can have one stdout read and one stdin write in flight. Provider preparation is serialized and
/// adds at most one stdout/stderr probe pair or one model connection pair.
const ADMITTED_PROVIDER_PIPE_OPERATIONS: usize = runtrol_daemon::MAX_BLOCKING_PROVIDER_OPERATIONS;

/// The maximum number of blocking workers this I/O supervisor may create.
///
/// Six workers above the admitted provider pipe operations leave progress capacity for filesystem and resolver work
/// without restoring Tokio's implicit 512-thread ceiling.
const MAX_BLOCKING_THREADS: usize = ADMITTED_PROVIDER_PIPE_OPERATIONS + 6;

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    if let Some(bootstrapped) = runtrol_childproc::bootstrap_if_requested(&words) {
        return match bootstrapped {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report(&format!("runtrol child bootstrap failed: {error}"));
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(target_os = "macos")]
    if words.first().map(String::as_str) == Some(runtrol_cli::DAEMON_ARGUMENT) {
        if let Err(error) = runtrol_childproc::handoff::prepare_macos_daemon_allocator(&words) {
            report(&format!(
                "runtrol cannot prepare its daemon allocator: {error}"
            ));
            return ExitCode::FAILURE;
        }
    }

    // Before anything could be started. Whether this program's own handles may travel to what it starts is a property
    // of the process, so it is set once here rather than argued about at each spawn. Measured: without it, a command
    // that starts a daemon hands that daemon a copy of the shell's own pipe, and the shell waits forever with nothing
    // to show for it.
    if let Err(error) = runtrol_childproc::keep_handles_to_ourselves() {
        report(&format!("runtrol cannot start: {error}"));
        return ExitCode::FAILURE;
    }

    match choose(&words) {
        Personality::Daemon => run(serving()),
        Personality::Endpoint => run(endpointing()),
        Personality::RuntimeLocator(prefer) => runtime_locating(prefer.as_deref()),
        Personality::Status { json } => run(status_reporting(json)),
        Personality::TerminalBridge {
            provider,
            arguments,
        } => run(bridging(&provider, &arguments)),
        // One question, one line, then gone: the machinery this answer needs costs the asking process
        // megabytes for its whole life, so the asking process is this one and its life is milliseconds.
        Personality::WhoHolds(path) => {
            if runtrol_childproc::held::run_who_holds(&path) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Personality::ProviderShims(directory) => run(shimming(&directory)),
        Personality::Administration(words) => run(administering(&words)),
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
            "runtrol <command>. try: endpoint, status, runtime-locator, integrations, requests, providers, list, start, resume, say, answer, stop, watch, close, legacy, panic"
                .to_owned(),
        ),
        // Spelled as a subcommand rather than inferred from how the program was invoked. Inferring it from the
        // executable's own name would mean a renamed file behaving differently, which is a surprise nobody asked for.
        Some(word) if word == runtrol_cli::DAEMON_ARGUMENT => Personality::Daemon,
        Some(word) if word == ENDPOINT_ARGUMENT => Personality::Endpoint,
        Some(word) if word == RUNTIME_LOCATOR_ARGUMENT => Personality::RuntimeLocator(
            match words.get(1..) {
                Some([flag, digest]) if flag == "--prefer" => Some(digest.clone()),
                _ => None,
            },
        ),
        Some(word) if word == STATUS_ARGUMENT => Personality::Status {
            json: words.get(1).is_some_and(|flag| flag == "--json"),
        },
        Some(word) if word == TERMINAL_BRIDGE_ARGUMENT => match words.get(1) {
            Some(provider) => Personality::TerminalBridge {
                provider: provider.clone(),
                arguments: words.get(2..).unwrap_or_default().to_vec(),
            },
            None => Personality::Usage("runtrol bridge <provider> [arguments...]".to_owned()),
        },
        Some("shims") => match words.get(1..) {
            Some([directory]) => Personality::ProviderShims(directory.into()),
            _ => Personality::Usage("runtrol shims <dedicated-directory>".to_owned()),
        },
        Some(word) if word == runtrol_childproc::held::HOLDER_SUBCOMMAND => match words.get(1) {
            Some(path) => Personality::WhoHolds(path.into()),
            None => Personality::Usage(format!("runtrol {word} <path>")),
        },
        Some(_) if runtrol_cli::is_administration(words) => {
            Personality::Administration(words.to_vec())
        }
        Some(_) => Personality::Command(words.to_vec()),
    }
}

/// Materialize the command names declared by every usable terminal provider.
fn shimming(directory: &std::path::Path) -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    move |runtime| {
        let Some((executable, address)) = own_generation() else {
            return ExitCode::FAILURE;
        };
        let providers = match runtime.block_on(runtrol_cli::bridge_providers(&address, &executable))
        {
            Ok(providers) => providers,
            Err(error) => {
                report(&error.to_string());
                return ExitCode::FAILURE;
            }
        };
        let declarations: Vec<_> = providers
            .iter()
            .map(|provider| runtrol_childproc::ProviderShim {
                provider: &provider.id,
                command_names: &provider.command_names,
            })
            .collect();
        match runtrol_childproc::materialize_provider_shims(directory, &executable, &declarations) {
            Ok(written) => {
                say(&format!(
                    "{} provider command shims are ready in {}",
                    written.len(),
                    directory.display(),
                ));
                ExitCode::SUCCESS
            }
            Err(error) => {
                report(&error.to_string());
                ExitCode::FAILURE
            }
        }
    }
}

/// Be the first local viewer of one daemon-owned provider terminal.
fn bridging(
    provider: &str,
    arguments: &[String],
) -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    move |runtime| {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            report(
                "a provider terminal bridge requires attached standard input and output terminals",
            );
            return ExitCode::FAILURE;
        }
        let terminal = match runtrol_childproc::LocalTerminal::acquire() {
            Ok(terminal) => terminal,
            Err(error) => {
                report(&format!("cannot acquire the invoking terminal: {error}"));
                return ExitCode::FAILURE;
            }
        };
        let initial = match terminal.size() {
            Ok(size) => size,
            Err(error) => {
                drop(terminal);
                report(&format!("cannot read the invoking terminal size: {error}"));
                return ExitCode::FAILURE;
            }
        };
        let here = match std::env::current_dir() {
            Ok(here) => here.to_string_lossy().into_owned(),
            Err(error) => {
                drop(terminal);
                report(&format!("cannot tell which directory this is: {error}"));
                return ExitCode::FAILURE;
            }
        };
        let Some((executable, address)) = own_generation() else {
            drop(terminal);
            return ExitCode::FAILURE;
        };
        let carried = runtime.block_on(runtrol_cli::bridge(
            &address,
            &executable,
            provider,
            arguments,
            &here,
            (initial.cols, initial.rows),
            || terminal.size().map(|size| (size.cols, size.rows)),
        ));
        drop(terminal);
        match carried {
            Ok(0) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(error) => {
                report(&error.to_string());
                ExitCode::FAILURE
            }
        }
    }
}

/// Run one owner-only Runtime administration command.
fn administering(words: &[String]) -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    move |runtime| {
        let Some((executable, address)) = own_generation() else {
            return ExitCode::FAILURE;
        };
        let standard_input = std::io::stdin();
        let standard_output = std::io::stdout();
        let interactive = standard_input.is_terminal() && standard_output.is_terminal();
        let mut input = standard_input.lock();
        let mut output = standard_output.lock();
        match runtime.block_on(runtrol_cli::administer(
            &address,
            &executable,
            words,
            interactive,
            &mut input,
            &mut output,
        )) {
            Ok(runtrol_cli::Outcome::Carried) => ExitCode::SUCCESS,
            Ok(runtrol_cli::Outcome::Refused) => ExitCode::FAILURE,
            Err(error) => {
                report(&error.to_string());
                ExitCode::FAILURE
            }
        }
    }
}

/// Run one personality on a runtime of its own.
///
/// Built here rather than by an attribute on `main`, because entry-point selection happens before the runtime exists.
/// A command is one connection and some waiting; a daemon supervises processes.
fn run<Work>(work: Work) -> ExitCode
where
    Work: FnOnce(&tokio::runtime::Runtime) -> ExitCode,
{
    match supervisor_runtime() {
        Ok(runtime) => work(&runtime),
        Err(error) => {
            report(&format!("runtrol cannot start: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Build the runtime shared by command, endpoint, and daemon personalities.
fn supervisor_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        // Blocking workers retire after one idle second instead of the default ten. The Unix spawn path and
        // every durable session write run on those workers now, and a worker that lingers is a thread stack
        // held by an idle daemon: on the macOS host that alone straddled the idle footprint budget, passing
        // or failing by about a mebibyte depending on the clock (measured 2026-08-28).
        .thread_keep_alive(std::time::Duration::from_secs(1))
        .enable_all()
        .build()
}

/// This executable and the control endpoint of the generation it serves under.
///
/// Both ends of the local connection derive the address from the same executable bytes and the same home, which
/// is what lets a command reach exactly the build it is, and start that build when nothing listens there.
fn own_generation() -> Option<(std::path::PathBuf, String)> {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            report(&format!("cannot tell where runtrol itself is: {error}"));
            return None;
        }
    };
    match runtrol_daemon::generation_endpoint(None, &executable) {
        Ok(endpoint) => Some((executable, endpoint)),
        Err(error) => {
            report(&format!(
                "cannot tell where this runtrol generation listens: {error}"
            ));
            None
        }
    }
}

/// Be the daemon: one generation, beside whatever generation was serving before.
fn serving() -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    |runtime| {
        let Some((_executable, address)) = own_generation() else {
            return ExitCode::FAILURE;
        };
        let identity = match runtrol_daemon::GenerationIdentity::of_this_executable() {
            Ok(identity) => identity,
            Err(error) => {
                report(&format!("runtrol cannot start a daemon: {error}"));
                return ExitCode::FAILURE;
            }
        };

        // The generation endpoint is bound first, so a command that started this daemon connects the moment
        // the pipe exists and waits for assembly rather than timing out. Composing then asks every earlier
        // generation to hand over the store and establishes containment before any child could exist.
        // A detached daemon's streams go nowhere, so from here on a panic lands in the home's bounded
        // crash file instead of evaporating with the process. Before assembly, because the handover to
        // this generation happens inside assembly and a failure there is the one most worth reading.
        match runtrol_daemon::crash_log_path(None) {
            Ok(path) => runtrol_daemon::record_panics_at(&path),
            Err(error) => {
                report(&format!("runtrol cannot start a daemon: {error}"));
                return ExitCode::FAILURE;
            }
        }
        let served = runtime.block_on(async move {
            boot_trace("daemon: binding the generation endpoint");
            let listener = runtrol_ipc::transport::Listener::bind(&address).await?;
            boot_trace("daemon: endpoint bound; assembling");
            let composed =
                runtrol_daemon::assemble_superseding(runtrol_drivers::builtin(), &identity)
                    .await
                    .map_err(|error| {
                        runtrol_daemon::ServeError::RuntimeBootstrap(error.to_string())
                    })?;
            boot_trace("daemon: assembled; serving");
            runtrol_daemon::serve(composed, listener).await
        });

        match served {
            Ok(()) => ExitCode::SUCCESS,
            // The ordinary reason for failing to listen is that this exact generation is already serving this
            // home, which is not a failure of anything: the command that started this one reaches that one instead.
            Err(error) => {
                report(&format!("runtrol stopped serving: {error}"));
                ExitCode::FAILURE
            }
        }
    }
}

/// Ensure this executable's generation is serving and report the exact address its local IPC clients must use.
///
/// The endpoint stays owned by `RuntrolHome` and the generation by the executable's bytes. A native surface asks
/// this executable once instead of reimplementing platform home selection, canonicalization, Windows
/// fingerprinting, Unix socket length rules, or the digest.
/// The control endpoint to use when this build's own generation is draining.
///
/// A draining generation refuses new conversations on purpose, and a client that keeps its address never
/// leaves it: the address is derived from the executable's own bytes, so asking again returns the same
/// draining generation forever. Measured 2026-08-26 on a window that had been open across an update, where
/// every new conversation was refused with the daemon's own words about generations.
///
/// Only that case redirects. A build whose generation is not running at all still starts it, because that is
/// how an update arrives: attaching to somebody else's generation instead would leave the new build forever
/// unstarted and the update forever unapplied.
fn endpoint_past_a_draining_own(
    runtime: &tokio::runtime::Runtime,
    own_address: &str,
) -> Option<String> {
    // A locator that cannot be read is not a reason to refuse to connect: the caller falls back to its own
    // address, which is what this function existed to improve on rather than depend on.
    let Ok(generations) = runtime.block_on(runtrol_daemon::status(None)) else {
        return None;
    };
    let own_is_draining = generations.iter().any(|status| {
        status.generation.control_endpoint == own_address
            && status.answering
            && status.generation.draining
    });
    if !own_is_draining {
        return None;
    }
    generations
        .iter()
        .filter(|status| status.answering && !status.generation.draining)
        .max_by_key(|status| status.generation.started_at_ms)
        .map(|status| status.generation.control_endpoint.clone())
}

fn endpointing() -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    |runtime| {
        let Some((executable, own)) = own_generation() else {
            return ExitCode::FAILURE;
        };
        let address = endpoint_past_a_draining_own(runtime, &own).unwrap_or(own);

        match runtime.block_on(runtrol_cli::reach(&address, &executable)) {
            Ok(connection) => {
                drop(connection);
                say(&address);
                ExitCode::SUCCESS
            }
            Err(error) => {
                report(&format!("cannot reach the runtrol daemon: {error}"));
                ExitCode::FAILURE
            }
        }
    }
}

/// Report the public locator only after the native client has validated its file type, bounds, ownership, DACL or
/// Unix mode, closed record, and endpoint confinement. The chosen generation is the preferred digest when it is
/// listed and not draining, otherwise the newest that is not draining.
fn runtime_locating(prefer: Option<&str>) -> ExitCode {
    use runtrol_runtime_client::{LocatorState, RuntimeLocator};

    let locator = match RuntimeLocator::system()
        .map(|candidate| match prefer {
            Some(digest) => candidate.preferring(digest),
            None => candidate,
        })
        .and_then(|candidate| candidate.inspect())
    {
        Ok(LocatorState::Running(locator)) => locator,
        Ok(LocatorState::NotInstalled) => {
            report("the Runtrol Runtime locator is not installed");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            report(&format!(
                "cannot validate the Runtrol Runtime locator: {error}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let encoded = serde_json::json!({
        "instanceId": locator.instance_id(),
        "endpoint": locator.endpoint(),
        "runtimeVersion": locator.runtime_version(),
        "digest": locator.digest(),
        "draining": locator.draining(),
    });
    say(&encoded.to_string());
    ExitCode::SUCCESS
}

/// Print every daemon generation of this home: which build, which process, how many turns still run there,
/// whether it is draining, and whether it answers right now. Starts nothing.
fn status_reporting(json: bool) -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    move |runtime| {
        let generations = match runtime.block_on(runtrol_daemon::status(None)) {
            Ok(generations) => generations,
            Err(error) => {
                report(&format!("cannot read the runtrol generations: {error}"));
                return ExitCode::FAILURE;
            }
        };
        if json {
            match serde_json::to_string(&generations) {
                Ok(encoded) => say(&encoded),
                Err(error) => {
                    report(&format!("cannot encode the runtrol generations: {error}"));
                    return ExitCode::FAILURE;
                }
            }
            return ExitCode::SUCCESS;
        }
        if generations.is_empty() {
            say("no runtrol generation is serving this home");
            return ExitCode::SUCCESS;
        }
        for status in generations {
            let generation = &status.generation;
            say(&format!(
                "{}  pid {}  v{}  started {}  live turns {}  {}  {}",
                generation.digest.get(..16).unwrap_or(&generation.digest),
                generation.process_id,
                generation.runtime_version,
                generation.started_at_ms,
                generation.live_sessions,
                if generation.draining {
                    "draining"
                } else {
                    "current"
                },
                if status.answering {
                    "answering"
                } else {
                    "not answering"
                },
            ));
        }
        ExitCode::SUCCESS
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

        // Where this executable's generation listens, and the program a daemon is started from when there is
        // none. Named rather than inferred inside the command surface: a library that ran "whatever process this
        // is" would run the test runner inside a test.
        let Some((executable, address)) = own_generation() else {
            return ExitCode::FAILURE;
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

/// One boot step on stderr, only when `RUNTROL_CLOSE_TRACE=1` asks for it (the CI harnesses do).
///
/// A daemon that never becomes ready and says nothing cannot be told apart from one that never ran
/// (measured 2026-08-27: two macOS gate failures whose captured daemon stderr was empty).
#[expect(
    clippy::print_stderr,
    reason = "the breadcrumb exists to reach a harness's captured stderr, and only when RUNTROL_CLOSE_TRACE=1 asks for it"
)]
fn boot_trace(step: &str) {
    if std::env::var_os("RUNTROL_CLOSE_TRACE").is_some_and(|value| value == "1") {
        eprintln!("runtrol {step}");
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
    #[cfg(windows)]
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use super::*;

    #[cfg(windows)]
    const PIPE_FIXTURE_ENV: &str = "RUNTROL_WINDOWS_PIPE_FIXTURE";
    #[cfg(windows)]
    const PIPE_FIXTURE_READY: &str = "runtrol-pipe-fixture-ready";

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

    #[test]
    fn endpoint_discovery_is_not_sent_to_the_daemon_as_a_product_request() {
        assert!(matches!(
            choose(&typed(ENDPOINT_ARGUMENT)),
            Personality::Endpoint
        ));
    }

    #[test]
    fn runtime_locator_validation_is_not_sent_as_a_product_request() {
        assert!(matches!(
            choose(&typed(RUNTIME_LOCATOR_ARGUMENT)),
            Personality::RuntimeLocator(None)
        ));
        match choose(&typed("runtime-locator --prefer abc")) {
            Personality::RuntimeLocator(Some(digest)) => assert_eq!(digest, "abc"),
            _ => panic!("expected the preferred digest to be read"),
        }
        assert!(matches!(
            choose(&typed("status --json")),
            Personality::Status { json: true }
        ));
        assert!(matches!(
            choose(&typed("status")),
            Personality::Status { json: false }
        ));
    }

    #[test]
    fn runtime_administration_is_not_sent_through_the_ordinary_command_parser() {
        for line in [
            "integrations list",
            "integrations review pending",
            "requests review pending",
            "providers help provider",
        ] {
            assert!(matches!(
                choose(&typed(line)),
                Personality::Administration(_)
            ));
        }
    }

    #[test]
    fn standalone_gui_is_not_an_entry_point() {
        let words = typed("gui");
        assert!(matches!(choose(&words), Personality::Command(_)));
        assert!(runtrol_cli::understand(&words, ".").is_err());
    }

    #[test]
    fn the_supervisor_runtime_has_one_async_thread() {
        let runtime = supervisor_runtime().expect("the product runtime builds");

        assert_eq!(
            runtime.handle().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread
        );
        assert_eq!(ADMITTED_PROVIDER_PIPE_OPERATIONS, 18);
        assert_eq!(MAX_BLOCKING_THREADS, 24);
    }

    #[test]
    fn admitted_provider_pipes_leave_blocking_progress_capacity() {
        let runtime = supervisor_runtime().expect("the product runtime builds");
        let release = Arc::new(AtomicBool::new(false));
        let (started, starts) = mpsc::channel();
        let mut occupied = Vec::new();

        for _ in 0..ADMITTED_PROVIDER_PIPE_OPERATIONS {
            let release = Arc::clone(&release);
            let started = started.clone();
            occupied.push(runtime.spawn_blocking(move || {
                started.send(()).expect("the test still observes starts");
                while !release.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
            }));
        }
        drop(started);

        let all_admitted_started = (0..ADMITTED_PROVIDER_PIPE_OPERATIONS)
            .all(|_| starts.recv_timeout(Duration::from_secs(2)).is_ok());
        let (spare_started, spare_starts) = mpsc::channel();
        for _ in ADMITTED_PROVIDER_PIPE_OPERATIONS..MAX_BLOCKING_THREADS {
            let release = Arc::clone(&release);
            let spare_started = spare_started.clone();
            occupied.push(runtime.spawn_blocking(move || {
                spare_started
                    .send(())
                    .expect("the test still observes spare starts");
                while !release.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
            }));
        }
        drop(spare_started);
        let spare_progressed = all_admitted_started
            && (ADMITTED_PROVIDER_PIPE_OPERATIONS..MAX_BLOCKING_THREADS)
                .all(|_| spare_starts.recv_timeout(Duration::from_secs(2)).is_ok());

        release.store(true, Ordering::Release);
        runtime.block_on(async {
            for worker in occupied {
                worker.await.expect("an occupied worker exits cleanly");
            }
        });

        assert!(
            all_admitted_started,
            "every provider pipe operation admitted by the daemon must get a worker"
        );
        assert!(
            spare_progressed,
            "provider pipe saturation must leave every reserved progress worker available"
        );
    }

    #[cfg(windows)]
    #[test]
    fn silent_pipe_fixture() {
        if std::env::var_os(PIPE_FIXTURE_ENV).is_none() {
            return;
        }
        let mut output = std::io::stdout();
        writeln!(output, "{PIPE_FIXTURE_READY}").expect("the fixture marker is writable");
        output.flush().expect("the fixture marker is visible");
        loop {
            std::thread::park();
        }
    }

    #[cfg(windows)]
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fault injection keeps child, pipe, worker, and cleanup lifetimes in one visible scope"
    )]
    fn windows_provider_pipes_leave_exact_progress_capacity() {
        use std::process::Stdio;

        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        struct Fixture {
            children: Vec<tokio::process::Child>,
            release: Arc<AtomicBool>,
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                self.release.store(true, Ordering::Release);
                for child in &mut self.children {
                    drop(child.start_kill());
                }
            }
        }

        fn child(stderr: Stdio) -> tokio::process::Child {
            let executable = std::env::current_exe().expect("the test executable has a path");
            let mut command = tokio::process::Command::new(executable);
            command
                .arg("--exact")
                .arg("tests::silent_pipe_fixture")
                .arg("--nocapture")
                .env(PIPE_FIXTURE_ENV, "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(stderr)
                .kill_on_drop(true);
            runtrol_childproc::hide_console_window(command.as_std_mut());
            command.spawn().expect("the silent pipe fixture starts")
        }

        let runtime = supervisor_runtime().expect("the product runtime builds");
        runtime.block_on(async {
            let release = Arc::new(AtomicBool::new(false));
            let mut fixture = Fixture {
                children: Vec::new(),
                release: Arc::clone(&release),
            };
            let mut pipe_tasks = Vec::new();
            let mut pipe_ready = Vec::new();
            let blocked_write = Arc::new(vec![0_u8; 4 * 1024 * 1024]);

            for _ in 0..(ADMITTED_PROVIDER_PIPE_OPERATIONS - 2) / 2 {
                let mut process = child(Stdio::null());
                let mut stdin = process.stdin.take().expect("hot stdin is piped");
                let stdout = process.stdout.take().expect("hot stdout is piped");
                let blocked_write = Arc::clone(&blocked_write);
                pipe_tasks.push(tokio::spawn(async move {
                    drop(stdin.write_all(&blocked_write).await);
                }));
                let (ready, readiness) = tokio::sync::oneshot::channel();
                pipe_tasks.push(tokio::spawn(async move {
                    let mut stdout = BufReader::new(stdout);
                    let mut line = String::new();
                    loop {
                        match stdout.read_line(&mut line).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) if line.contains(PIPE_FIXTURE_READY) => break,
                            Ok(_) => line.clear(),
                        }
                    }
                    let _ready = ready.send(());
                    let mut byte = [0_u8; 1];
                    drop(stdout.read(&mut byte).await);
                }));
                pipe_ready.push(readiness);
                fixture.children.push(process);
            }

            let mut preparation = child(Stdio::piped());
            drop(preparation.stdin.take());
            let stdout = preparation
                .stdout
                .take()
                .expect("preparation stdout is piped");
            let mut stderr = preparation
                .stderr
                .take()
                .expect("preparation stderr is piped");
            let (ready, readiness) = tokio::sync::oneshot::channel();
            pipe_tasks.push(tokio::spawn(async move {
                let mut stdout = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    match stdout.read_line(&mut line).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) if line.contains(PIPE_FIXTURE_READY) => break,
                        Ok(_) => line.clear(),
                    }
                }
                let _ready = ready.send(());
                let mut byte = [0_u8; 1];
                drop(stdout.read(&mut byte).await);
            }));
            pipe_ready.push(readiness);
            pipe_tasks.push(tokio::spawn(async move {
                let mut byte = [0_u8; 1];
                drop(stderr.read(&mut byte).await);
            }));
            fixture.children.push(preparation);

            for readiness in pipe_ready {
                tokio::time::timeout(Duration::from_secs(2), readiness)
                    .await
                    .expect("a fixture announced readiness")
                    .expect("a fixture kept its stdout open");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut sentinels = Vec::new();
            let mut starts = Vec::new();
            for _ in ADMITTED_PROVIDER_PIPE_OPERATIONS..MAX_BLOCKING_THREADS {
                let release = Arc::clone(&release);
                let (started, start) = tokio::sync::oneshot::channel();
                sentinels.push(tokio::task::spawn_blocking(move || {
                    let _started = started.send(());
                    while !release.load(Ordering::Acquire) {
                        std::thread::park_timeout(Duration::from_millis(10));
                    }
                }));
                starts.push(start);
            }

            let mut all_spares_started = true;
            for start in starts {
                if tokio::time::timeout(Duration::from_secs(2), start)
                    .await
                    .is_err()
                {
                    all_spares_started = false;
                }
            }

            let seventh_release = Arc::clone(&release);
            let (seventh_started, mut seventh_start) = tokio::sync::oneshot::channel();
            let seventh = tokio::task::spawn_blocking(move || {
                let _started = seventh_started.send(());
                while !seventh_release.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
            });
            let seventh_was_queued =
                tokio::time::timeout(Duration::from_millis(250), &mut seventh_start)
                    .await
                    .is_err();

            let released_child = fixture
                .children
                .first_mut()
                .expect("the fixture has hot children");
            drop(released_child.start_kill());
            let seventh_progressed = if seventh_was_queued {
                tokio::time::timeout(Duration::from_secs(2), seventh_start)
                    .await
                    .is_ok()
            } else {
                false
            };

            release.store(true, Ordering::Release);
            for child in &mut fixture.children {
                drop(child.start_kill());
                drop(child.wait().await);
            }
            for task in pipe_tasks {
                drop(task.await);
            }
            for sentinel in sentinels {
                sentinel.await.expect("a spare sentinel exits");
            }
            seventh.await.expect("the queued sentinel exits");

            assert!(
                all_spares_started,
                "all six progress workers must start beside eighteen provider pipe operations"
            );
            assert!(
                seventh_was_queued,
                "a twenty-fifth blocking operation must wait at the configured ceiling"
            );
            assert!(
                seventh_progressed,
                "a queued blocking operation must start when one provider pipe closes"
            );
        });
    }
}
