//! A parent that establishes containment, starts a long-lived child, and then waits to be killed.
//!
//! Exists so that `tests/containment.rs` can prove the guarantee rather than assert it. The test cannot check
//! containment from inside its own process: proving "children die when the parent is killed without warning"
//! needs a parent that can be killed without warning, and a test harness is not that.
//!
//! Prints the tracked root and its descendant process ids on stdout, then blocks. The test reads both ids, kills
//! this process the hard way, and checks whether the complete process group is still there.
//!
//! # Why the child is another copy of this binary
//!
//! The first version used the platform's own sleep tool, and the child exited immediately: the Windows one
//! needs a console and the test had given it a null stdin. Re-running this binary in a sleeping mode removes
//! every assumption about what is installed and how it behaves when its input is redirected, which is exactly
//! the kind of dependency a test about process lifetime should not have.

// A test helper's whole output is a line the test reads. It is never linked into the daemon.
#![expect(
    clippy::print_stdout,
    reason = "the printed process id is this program's entire interface to the test that runs it"
)]

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// The argument that puts this binary into its sleeping mode.
const SLEEP_MODE: &str = "--sleep-until-killed";

/// A descendant that shares the tracked root's process group and only blocks.
const LEAF_MODE: &str = "--leaf-until-killed";

/// Exit immediately with the supplied code so integration tests can verify keeper status proxying.
const EXIT_MODE: &str = "--exit-with";

/// Establish tracked containment and recover whatever the killed parent left.
const RECOVER_MODE: &str = "--recover";

/// Name the guard directory for the supervising-parent mode.
const GUARD_DIRECTORY: &str = "--guard-directory";

/// Long enough that the test finishes first, short enough that a stray copy cannot linger.
const SLEEP: Duration = Duration::from_mins(1);

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    if let Some(result) = runtrol_childproc::bootstrap_if_requested(&words) {
        if let Err(error) = result {
            eprintln!("child bootstrap failed: {error}");
            std::process::exit(8);
        }
        return;
    }

    if words.first().is_some_and(|argument| argument == LEAF_MODE) {
        std::thread::sleep(SLEEP);
        return;
    }

    if words.first().is_some_and(|argument| argument == EXIT_MODE) {
        let code = match words.get(1).map(|value| value.parse::<i32>()) {
            Some(Ok(code)) => code,
            Some(Err(_)) | None => 20,
        };
        std::process::exit(code);
    }

    if words.first().is_some_and(|argument| argument == SLEEP_MODE) {
        sleeping_root();
    }

    if words
        .first()
        .is_some_and(|argument| argument == RECOVER_MODE)
    {
        let Some(directory) = words.get(1) else {
            eprintln!("recovery needs a guard directory");
            std::process::exit(9);
        };
        if let Err(error) = runtrol_childproc::Containment::establish_tracked(Path::new(directory))
        {
            eprintln!("could not recover containment: {error}");
            std::process::exit(10);
        }
        return;
    }

    let Some(directory) = words
        .first()
        .filter(|argument| argument.as_str() == GUARD_DIRECTORY)
        .and_then(|_| words.get(1))
    else {
        eprintln!("the parent needs --guard-directory <path>");
        std::process::exit(11);
    };
    supervising_parent(directory);
}

fn sleeping_root() -> ! {
    let own_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("could not find the descendant binary: {error}");
            std::process::exit(13);
        }
    };
    let mut descendant = std::process::Command::new(own_path);
    descendant.arg(LEAF_MODE);
    descendant.stdin(Stdio::null());
    descendant.stdout(Stdio::null());
    descendant.stderr(Stdio::null());
    runtrol_childproc::hide_console_window(&mut descendant);
    let descendant = match descendant.spawn() {
        Ok(descendant) => descendant,
        Err(error) => {
            eprintln!("could not start the descendant: {error}");
            std::process::exit(14);
        }
    };
    println!("{}", descendant.id());
    if let Err(error) = std::io::stdout().flush() {
        eprintln!("could not flush the descendant id: {error}");
        std::process::exit(15);
    }
    std::thread::sleep(SLEEP);
    std::process::exit(0);
}

fn supervising_parent(directory: &str) -> ! {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("could not build the containment runtime: {error}");
            std::process::exit(19);
        }
    };
    let containment = match runtrol_childproc::Containment::establish_tracked(Path::new(directory))
    {
        Ok(containment) => containment,
        Err(error) => {
            eprintln!("could not establish containment: {error}");
            std::process::exit(2);
        }
    };

    let own_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("could not find this binary: {error}");
            std::process::exit(3);
        }
    };

    let mut command = runtrol_childproc::TrackedCommand::new(own_path);
    command.arg(SLEEP_MODE);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());
    let spawned = {
        let _entered = runtime.enter();
        command.spawn(&containment)
    };
    let (mut child, _child_guard) = match spawned {
        Ok(spawned) => spawned,
        Err(error) => {
            eprintln!("could not start the child: {error}");
            std::process::exit(4);
        }
    };

    let Some(child_id) = child.id() else {
        eprintln!("the child process has no identifier");
        std::process::exit(12);
    };
    let Some(child_stdout) = child.stdout.take() else {
        eprintln!("the child process has no output pipe");
        std::process::exit(16);
    };
    let descendant_id = match runtime.block_on(async {
        use tokio::io::AsyncBufReadExt as _;

        let mut descendant_lines = tokio::io::BufReader::new(child_stdout).lines();
        descendant_lines.next_line().await
    }) {
        Ok(Some(id)) => id,
        Err(error) => {
            eprintln!("could not read the descendant id: {error}");
            std::process::exit(17);
        }
        Ok(None) => {
            eprintln!("the child did not report its descendant");
            std::process::exit(18);
        }
    };
    println!("{child_id}");
    println!("{descendant_id}");
    if let Err(error) = std::io::stdout().flush() {
        eprintln!("could not flush the child id: {error}");
        std::process::exit(5);
    }

    // Block until killed. The containment guard stays alive for exactly as long as this process does, which is
    // the property under test.
    std::thread::sleep(SLEEP);
    std::process::exit(0);
}
