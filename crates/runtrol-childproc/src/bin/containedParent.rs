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

/// Exercise provider exit-status proxying from a binary that owns the bootstrap entry point.
#[cfg(unix)]
const VERIFY_EXIT_STATUS: &str = "--verify-exit-status";

/// Exercise an explicit keeper stop from a binary that owns the bootstrap entry point.
#[cfg(unix)]
const VERIFY_EXPLICIT_STOP: &str = "--verify-explicit-stop";

/// Exercise the panic stop path against more than one keeper.
#[cfg(unix)]
const VERIFY_STOP_ALL: &str = "--verify-stop-all";

/// Exercise provider spawn failure cleanup after the keeper has published its durable record.
#[cfg(unix)]
const VERIFY_SPAWN_FAILURE: &str = "--verify-spawn-failure";

/// Exercise session spawn and close after an update renamed a new build over the running image.
#[cfg(unix)]
const VERIFY_UPDATE_RENAME: &str = "--verify-update-rename";

/// Marks the disposable copy of this binary that the update-rename verification renames over.
#[cfg(unix)]
const UPDATE_RENAME_COPY_ENV: &str = "RUNTROL_CONTAINMENT_UPDATE_RENAME_COPY";

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

    #[cfg(unix)]
    if let Some(mode) = words.first().filter(|argument| {
        matches!(
            argument.as_str(),
            VERIFY_EXIT_STATUS
                | VERIFY_EXPLICIT_STOP
                | VERIFY_STOP_ALL
                | VERIFY_SPAWN_FAILURE
                | VERIFY_UPDATE_RENAME
        )
    }) {
        let Some(directory) = words.get(1) else {
            eprintln!("keeper verification needs a guard directory");
            std::process::exit(21);
        };
        let verified = match mode.as_str() {
            VERIFY_EXIT_STATUS => verify_exit_status(directory),
            VERIFY_EXPLICIT_STOP => verify_explicit_stop(directory),
            VERIFY_STOP_ALL => verify_stop_all(directory),
            VERIFY_SPAWN_FAILURE => verify_spawn_failure(directory),
            VERIFY_UPDATE_RENAME => verify_update_rename(directory),
            _ => Err("the keeper verification mode was not recognized".to_owned()),
        };
        if let Err(error) = verified {
            eprintln!("keeper verification failed: {error}");
            std::process::exit(22);
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
    let spawned = runtime.block_on(command.spawn(&containment));
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

#[cfg(unix)]
fn verification_context(
    directory: &str,
) -> Result<
    (
        tokio::runtime::Runtime,
        runtrol_childproc::Containment,
        std::path::PathBuf,
    ),
    String,
> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not build the containment runtime: {error}"))?;
    let containment = runtrol_childproc::Containment::establish_tracked(Path::new(directory))
        .map_err(|error| format!("could not establish containment: {error}"))?;
    let own_path = std::env::current_exe()
        .map_err(|error| format!("could not find the keeper verification binary: {error}"))?;
    Ok((runtime, containment, own_path))
}

#[cfg(unix)]
fn verify_exit_status(directory: &str) -> Result<(), String> {
    let (runtime, containment, own_path) = verification_context(directory)?;
    if containment.strength() != runtrol_childproc::Strength::EvenIfKilled {
        return Err("tracked Unix containment did not claim crash-safe strength".to_owned());
    }
    let mut command = runtrol_childproc::TrackedCommand::new(own_path);
    command.args([EXIT_MODE, "23"]);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::inherit());
    command.kill_on_drop(true);
    let spawned = runtime.block_on(command.spawn(&containment));
    let (mut child, mut guard) =
        spawned.map_err(|error| format!("could not start the exiting provider: {error}"))?;
    let status = runtime
        .block_on(child.wait())
        .map_err(|error| format!("could not read the provider exit status: {error}"))?;
    if status.code() != Some(23) {
        return Err(format!(
            "the keeper returned the wrong provider status: {status}"
        ));
    }
    guard
        .complete()
        .map_err(|error| format!("could not complete the natural exit guard: {error}"))?;
    verify_no_guard_records(directory)
}

#[cfg(unix)]
fn verify_explicit_stop(directory: &str) -> Result<(), String> {
    let (runtime, containment, own_path) = verification_context(directory)?;
    let mut command = runtrol_childproc::TrackedCommand::new(own_path);
    command.arg(LEAF_MODE);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::inherit());
    command.kill_on_drop(true);
    let spawned = runtime.block_on(command.spawn(&containment));
    let (mut child, mut guard) =
        spawned.map_err(|error| format!("could not start the long-lived provider: {error}"))?;
    runtime
        .block_on(guard.terminate(&mut child))
        .map_err(|error| format!("could not terminate the provider through its keeper: {error}"))?;
    verify_no_guard_records(directory)
}

#[cfg(unix)]
fn verify_stop_all(directory: &str) -> Result<(), String> {
    let (runtime, containment, own_path) = verification_context(directory)?;
    let mut children = Vec::new();
    let mut guards = Vec::new();
    for _ in 0..2 {
        let mut command = runtrol_childproc::TrackedCommand::new(&own_path);
        command.arg(LEAF_MODE);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::inherit());
        command.kill_on_drop(true);
        let spawned = {
            let _entered = runtime.enter();
            command.spawn(&containment)
        };
        let (child, guard) =
            spawned.map_err(|error| format!("could not start a stop-all provider: {error}"))?;
        children.push(child);
        guards.push(guard);
    }
    containment
        .terminate_all()
        .map_err(|error| format!("could not stop every registered keeper: {error}"))?;
    for child in &mut children {
        match runtime.block_on(child.wait()) {
            Ok(status) => {
                return Err(format!(
                    "a stopped keeper unexpectedly returned provider status {status}"
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
            Err(error) => {
                return Err(format!(
                    "a stopped keeper returned the wrong missing-frame error: {error}"
                ));
            }
        }
    }
    drop(guards);
    verify_no_guard_records(directory)
}

#[cfg(unix)]
fn verify_spawn_failure(directory: &str) -> Result<(), String> {
    let (runtime, containment, _own_path) = verification_context(directory)?;
    let mut command =
        runtrol_childproc::TrackedCommand::new(Path::new(directory).join("absent-provider"));
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::inherit());
    command.kill_on_drop(true);
    let spawned = runtime.block_on(command.spawn(&containment));
    let error = match spawned {
        Ok(_) => return Err("an absent provider unexpectedly started".to_owned()),
        Err(error) => error.to_string(),
    };
    if !error.contains("executing the supervised provider") {
        return Err(format!(
            "provider spawn failure lost its diagnostic: {error}"
        ));
    }
    verify_no_guard_records(directory)
}

/// Prove sessions still spawn and close after an update replaced the file behind this process.
///
/// Runs in two stages. The shared cargo test binary must stay where cargo put it, so the outer
/// stage copies itself into a disposable name beside the guard directory and re-executes that copy.
/// The copy performs the journey: one session before the update, then the autoUpdate rename
/// dance against its own image (hard link keeper, new build renamed over the running name), then a
/// second session that must start and close exactly like the first. Before the identity redesign,
/// the second spawn failed on Linux because the process re-found itself through a lookup that now
/// names a deleted path (the confirmed "갱신했더니 세션이 안 열린다" defect).
///
/// On macOS there is no descriptor-based spawn path, so this journey proves the weaker fact that a
/// same-content new build under the original name keeps sessions working; the deleted-lookup hazard
/// itself is Linux-measured.
#[cfg(unix)]
fn verify_update_rename(directory: &str) -> Result<(), String> {
    if std::env::var_os(UPDATE_RENAME_COPY_ENV).is_none() {
        let own_path = std::env::current_exe()
            .map_err(|error| format!("could not find the update-rename binary: {error}"))?;
        let parent = Path::new(directory).parent().ok_or_else(|| {
            "the guard directory has no parent for the disposable copy".to_owned()
        })?;
        let copy = parent.join("updateRenameCopy");
        std::fs::copy(&own_path, &copy)
            .map_err(|error| format!("could not copy the update-rename binary: {error}"))?;
        let status = std::process::Command::new(&copy)
            .args([VERIFY_UPDATE_RENAME, directory])
            .env(UPDATE_RENAME_COPY_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|error| format!("could not run the disposable update-rename copy: {error}"))?;
        for stray in ["updateRenameCopy", "updateRenameCopy.inuse"] {
            // The keeper link parallels the update design's inuse name; both are this stage's own
            // scratch and a missing one only means the failing stage never created it.
            if let Err(error) = std::fs::remove_file(parent.join(stray))
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!(
                    "could not remove the disposable copy {stray}: {error}"
                ));
            }
        }
        if !status.success() {
            return Err(format!("the update-rename journey failed with {status}"));
        }
        return Ok(());
    }

    let (runtime, containment, own_path) = verification_context(directory)?;
    let run_session = |label: &str| -> Result<(), String> {
        let mut command = runtrol_childproc::TrackedCommand::new(&own_path);
        command.args([EXIT_MODE, "0"]);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::inherit());
        command.kill_on_drop(true);
        let spawned = {
            let _entered = runtime.enter();
            command.spawn(&containment)
        };
        let (mut child, mut guard) =
            spawned.map_err(|error| format!("could not start the {label} session: {error}"))?;
        let status = runtime
            .block_on(child.wait())
            .map_err(|error| format!("could not read the {label} session status: {error}"))?;
        if status.code() != Some(0) {
            return Err(format!("the {label} session ended with {status}"));
        }
        guard
            .complete()
            .map_err(|error| format!("could not complete the {label} session guard: {error}"))
    };

    run_session("pre-update")?;

    // The update dance from the autoUpdate design, aimed at this running copy: keep the live
    // image reachable through a keeper link, then rename a new build over the only public name.
    let keeper = own_path.with_extension("inuse");
    std::fs::hard_link(&own_path, &keeper)
        .map_err(|error| format!("could not link the in-use image: {error}"))?;
    let incoming = own_path.with_extension("incoming");
    std::fs::copy(&own_path, &incoming)
        .map_err(|error| format!("could not stage the new build: {error}"))?;
    std::fs::rename(&incoming, &own_path).map_err(|error| {
        format!("could not rename the new build over the running image: {error}")
    })?;

    run_session("post-update")?;
    verify_no_guard_records(directory)
}

#[cfg(unix)]
fn verify_no_guard_records(directory: &str) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("could not read the guard directory: {error}"))?;
    for entry in entries {
        let name = entry
            .map_err(|error| format!("could not read a guard entry: {error}"))?
            .file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".pending") || name.ends_with(".active") {
            return Err(format!("failed provider spawn retained guard {name}"));
        }
    }
    Ok(())
}
