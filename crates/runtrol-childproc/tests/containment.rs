//! Proof that a killed parent takes its children with it.
//!
//! The one claim in this crate that cannot be checked from inside a unit test: showing that children die when
//! the parent is killed **without warning** needs a parent that can be killed without warning, and a test
//! harness is not that. So this drives a real helper process, kills it the hard way, and looks at whether the
//! grandchild is still there.
//!
//! Windows must remove the grandchild through its kernel job. On Unix the stable keeper must observe control EOF and
//! kill its own process group before a replacement supervisor validates and clears the stale guard. A surviving child
//! is red everywhere.

// The `allow-panic-in-tests` hatch covers `#[test]` functions and `#[cfg(test)]` modules, and a free helper
// in an integration test file is neither: measured, and noted in `clippy.toml`. A helper that cannot ask the
// operating system whether a process exists has nothing useful to return, and a wrong answer here would make
// the containment proof meaningless, so it panics and says so.
#![expect(
    clippy::panic,
    reason = "a test helper that cannot observe the system must stop the test, not guess at an answer"
)]

use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the kernel to reap the grandchild.
///
/// Generous. The kill is asynchronous, and a tight bound would make this fail on a loaded machine, which is
/// the worst kind of flake: one that looks like the guarantee broke.
const REAP_BUDGET: Duration = Duration::from_secs(10);

#[cfg(unix)]
#[test]
fn stable_keeper_proxies_the_provider_exit_status() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = guard_directory("exit-status");
    let status = Command::new(helper)
        .arg("--verify-exit-status")
        .arg(&guard_directory)
        .status()
        .expect("the exit-status keeper verification starts");
    assert!(status.success());
    std::fs::remove_dir_all(&guard_directory).expect("remove the exit-status guard directory");
}

#[cfg(unix)]
#[test]
fn explicit_termination_uses_the_keeper_control() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = guard_directory("explicit-stop");
    let status = Command::new(helper)
        .arg("--verify-explicit-stop")
        .arg(&guard_directory)
        .status()
        .expect("the explicit-stop keeper verification starts");
    assert!(status.success());
    std::fs::remove_dir_all(&guard_directory).expect("remove the explicit-stop guard directory");
}

#[cfg(unix)]
#[test]
fn an_update_renamed_over_the_running_image_and_sessions_still_open_and_close() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = guard_directory("update-rename");
    let status = Command::new(helper)
        .arg("--verify-update-rename")
        .arg(&guard_directory)
        .status()
        .expect("the update-rename keeper verification starts");
    assert!(status.success());
    std::fs::remove_dir_all(&guard_directory).expect("remove the update-rename guard directory");
}

#[cfg(unix)]
#[test]
fn provider_spawn_failure_removes_pending_and_active_guards() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = guard_directory("spawn-failure");
    let status = Command::new(helper)
        .arg("--verify-spawn-failure")
        .arg(&guard_directory)
        .status()
        .expect("the spawn-failure keeper verification starts");
    assert!(status.success());
    std::fs::remove_dir_all(&guard_directory).expect("remove the failed-spawn guard directory");
}

#[cfg(unix)]
#[test]
fn panic_termination_attempts_every_registered_keeper() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = guard_directory("stop-all");
    let status = Command::new(helper)
        .arg("--verify-stop-all")
        .arg(&guard_directory)
        .status()
        .expect("the stop-all keeper verification starts");
    assert!(status.success());
    std::fs::remove_dir_all(&guard_directory).expect("remove the stop-all guard directory");
}

#[test]
fn killing_the_parent_kills_the_child() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let guard_directory = std::env::temp_dir().join(format!(
        "runtrol-containment-recovery-{}",
        std::process::id()
    ));
    if guard_directory.exists() {
        std::fs::remove_dir_all(&guard_directory).expect("clear the previous guard directory");
    }
    std::fs::create_dir_all(&guard_directory).expect("create the guard directory");
    let guard_directory = guard_directory
        .canonicalize()
        .expect("canonicalize the guard directory");

    let mut command = Command::new(helper);
    command.arg("--guard-directory").arg(&guard_directory);
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());
    runtrol_childproc::hide_console_window(&mut command);
    let parent = command.spawn().expect("the helper binary must start");
    let mut cleanup = Cleanup {
        parent: Some(parent),
        helper: PathBuf::from(helper),
        guard_directory: guard_directory.clone(),
        process_ids: Vec::new(),
        finished: false,
    };

    let stdout = cleanup
        .parent
        .as_mut()
        .and_then(|parent| parent.stdout.take())
        .expect("stdout was piped");
    let mut lines = BufReader::new(stdout).lines();
    let root_reported = lines
        .next()
        .expect("the helper prints the tracked root id before blocking")
        .expect("the id line is readable");
    let tracked_root: u32 = root_reported
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("expected a process id, got {root_reported:?}: {error}"));
    let descendant_reported = lines
        .next()
        .expect("the helper prints the descendant id before blocking")
        .expect("the descendant id line is readable");
    let descendant: u32 = descendant_reported.trim().parse().unwrap_or_else(|error| {
        panic!("expected a process id, got {descendant_reported:?}: {error}")
    });
    cleanup.process_ids.extend([tracked_root, descendant]);

    assert!(
        is_alive(tracked_root),
        "the tracked root should be running before the parent is killed"
    );
    assert!(
        is_alive(descendant),
        "the tracked root's descendant should be running before the parent is killed"
    );

    // The hard way. No signal handler runs, no destructor runs, nothing gets a chance to clean up. That is
    // the case the containment exists for.
    cleanup.kill_parent().expect("the helper must be killable");

    let strength = runtrol_childproc::Containment::platform_strength();
    if strength.survives_an_unclean_kill() {
        let recovered = cleanup
            .recover()
            .expect("the replacement supervisor must start");
        assert!(
            recovered.success(),
            "the replacement supervisor did not recover the process group"
        );
    }

    assert!(
        wait_until_gone(tracked_root),
        "the tracked root survived both the hard parent kill and the platform's required recovery boundary"
    );
    assert!(
        wait_until_gone(descendant),
        "the tracked root's descendant survived the platform's required process-group recovery boundary"
    );
    cleanup
        .finish()
        .expect("remove the recovered guard directory");
}

#[cfg(unix)]
#[expect(
    clippy::expect_used,
    reason = "a test fixture cannot continue with an absent or ambiguous private guard directory"
)]
fn guard_directory(label: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "runtrol-containment-{label}-{}",
        std::process::id()
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).expect("clear the previous guard directory");
    }
    std::fs::create_dir_all(&directory).expect("create the guard directory");
    directory
        .canonicalize()
        .expect("canonicalize the guard directory")
}

struct Cleanup {
    parent: Option<Child>,
    helper: PathBuf,
    guard_directory: PathBuf,
    process_ids: Vec<u32>,
    finished: bool,
}

impl Cleanup {
    fn kill_parent(&mut self) -> std::io::Result<()> {
        let Some(mut parent) = self.parent.take() else {
            return Ok(());
        };
        if parent.try_wait()?.is_none() {
            parent.kill()?;
        }
        let _status = parent.wait()?;
        Ok(())
    }

    fn recover(&self) -> std::io::Result<std::process::ExitStatus> {
        let mut command = Command::new(&self.helper);
        command.arg("--recover").arg(&self.guard_directory);
        runtrol_childproc::hide_console_window(&mut command);
        command.status()
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if self.guard_directory.exists() {
            std::fs::remove_dir_all(&self.guard_directory)?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _stopped = self.kill_parent();
        #[cfg(unix)]
        if self.guard_directory.exists() {
            let _recovered = self.recover();
        }
        for pid in &self.process_ids {
            if is_alive(*pid) {
                force_kill(*pid);
            }
        }
        for pid in &self.process_ids {
            let _gone = wait_until_gone(*pid);
        }
        if self.guard_directory.exists() {
            let _removed = std::fs::remove_dir_all(&self.guard_directory);
        }
    }
}

#[cfg(windows)]
fn force_kill(pid: u32) {
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/F"]);
    runtrol_childproc::hide_console_window(&mut command);
    let _status = command.status();
}

#[cfg(unix)]
fn force_kill(pid: u32) {
    let _status = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
}

/// Wait for a process to disappear, up to [`REAP_BUDGET`].
fn wait_until_gone(pid: u32) -> bool {
    let deadline = Instant::now() + REAP_BUDGET;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !is_alive(pid)
}

/// Whether a process with this id is still running.
///
/// Asks the operating system's own tool rather than a crate. One process per poll is more than a system call
/// would cost, and this runs a handful of times in one test, so the cheaper thing to get right wins.
#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    let mut command = Command::new("tasklist");
    command.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
    runtrol_childproc::hide_console_window(&mut command);
    let output = command.output();
    match output {
        // `tasklist` prints an informational line rather than failing when nothing matches, so presence is
        // decided by whether the id itself comes back.
        Ok(output) => String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()),
        Err(error) => panic!("cannot ask the operating system about process {pid}: {error}"),
    }
}

/// Whether a process with this id is still running.
#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output();
    match output {
        Ok(output) => {
            output.status.success()
                && !String::from_utf8_lossy(&output.stdout)
                    .trim_start()
                    .starts_with('Z')
        }
        Err(error) => panic!("cannot ask the operating system about process {pid}: {error}"),
    }
}

/// The kernel's own answer to "is this process inside the containment": a leaf the helper started is, and the
/// test process that started the helper is not.
#[test]
fn the_containment_says_who_is_inside_it() {
    let helper = env!("CARGO_BIN_EXE_containedParent");
    let mut command = Command::new(helper);
    command
        .arg("--report-membership")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    runtrol_childproc::hide_console_window(&mut command);
    let output = command.output().expect("the membership report runs");
    assert!(
        output.status.success(),
        "the membership report failed: {}",
        output.status
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(report.trim(), "inside=true outside=false", "{report}");
}
