//! The runtrol executable. It picks a personality from argv and does nothing else.
//!
//! Logic does not go here. This file necessarily links every crate (one executable, three personalities), so it is the
//! one place where every architectural boundary is invisible. Keeping it empty of decisions is what keeps those
//! boundaries meaningful: the command surface is a crate that cannot see storage, the daemon is a crate a test can
//! link, and this is neither.
//!
//! # Four personalities
//!
//! - **A daemon**, asked for by name, which serves until it is stopped.
//! - **A command**, which is everything else somebody types.
//! - **A local endpoint reporter**, used by native surfaces that speak the framed IPC directly.
//! - **A daemon started by a command**, which is the first one again, launched by the second when nothing is
//!   listening. It is not a fifth thing to build: it is why the first is a subcommand rather than a separate program.
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
    /// Ensure the daemon is reachable and print its exact local endpoint.
    Endpoint,
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

/// The word native surfaces use to discover the daemon endpoint from its single owner.
const ENDPOINT_ARGUMENT: &str = "endpoint";

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
        Personality::Endpoint => run(endpointing()),
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
            "runtrol <command>. try: gui, endpoint, list, start, resume, say, answer, stop, watch, close, consult, panic"
                .to_owned(),
        ),
        // Spelled as a subcommand rather than inferred from how the program was invoked. Inferring it from the
        // executable's own name would mean a renamed file behaving differently, which is a surprise nobody asked for.
        Some(word) if word == runtrol_cli::DAEMON_ARGUMENT => Personality::Daemon,
        Some(word) if word == WINDOW_ARGUMENT => Personality::Window,
        Some(word) if word == ENDPOINT_ARGUMENT => Personality::Endpoint,
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
    match supervisor_runtime() {
        Ok(runtime) => work(&runtime),
        Err(error) => {
            report(&format!("runtrol cannot start: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Build the runtime shared by command and daemon personalities.
fn supervisor_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .enable_all()
        .build()
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
        // A detached daemon's streams go nowhere, so from here on a panic also lands in the home's
        // bounded crash file instead of evaporating with the process.
        runtrol_daemon::record_panics_at(composed.home.paths().daemon_crash_log().as_std_path());
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

/// Ensure one daemon exists and report the exact address its local IPC clients must use.
///
/// The endpoint stays owned by `RuntrolHome`. A native surface asks this executable once instead of reimplementing
/// platform home selection, canonicalization, Windows fingerprinting, or Unix socket length rules.
fn endpointing() -> impl FnOnce(&tokio::runtime::Runtime) -> ExitCode {
    |runtime| {
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
