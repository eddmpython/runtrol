//! Proof that a killed parent takes its children with it.
//!
//! The one claim in this crate that cannot be checked from inside a unit test: showing that children die when
//! the parent is killed **without warning** needs a parent that can be killed without warning, and a test
//! harness is not that. So this drives a real helper process, kills it the hard way, and looks at whether the
//! grandchild is still there.
//!
//! On the platform where containment is a kernel guarantee, the grandchild must be gone. On the platform where
//! it is not, this test says so rather than failing, because a test that fails for a documented platform
//! limitation trains people to ignore red.

// The `allow-panic-in-tests` hatch covers `#[test]` functions and `#[cfg(test)]` modules, and a free helper
// in an integration test file is neither: measured, and noted in `clippy.toml`. A helper that cannot ask the
// operating system whether a process exists has nothing useful to return, and a wrong answer here would make
// the containment proof meaningless, so it panics and says so.
#![expect(
    clippy::panic,
    reason = "a test helper that cannot observe the system must stop the test, not guess at an answer"
)]

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the kernel to reap the grandchild.
///
/// Generous. The kill is asynchronous, and a tight bound would make this fail on a loaded machine, which is
/// the worst kind of flake: one that looks like the guarantee broke.
const REAP_BUDGET: Duration = Duration::from_secs(10);

#[test]
fn killing_the_parent_kills_the_child() {
    let helper = env!("CARGO_BIN_EXE_containedParent");

    let mut parent = Command::new(helper)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the helper binary must start");

    let stdout = parent.stdout.take().expect("stdout was piped");
    let mut lines = BufReader::new(stdout).lines();
    let reported = lines
        .next()
        .expect("the helper prints the child id before blocking")
        .expect("the id line is readable");
    let grandchild: u32 = reported
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("expected a process id, got {reported:?}: {error}"));

    assert!(
        is_alive(grandchild),
        "the grandchild should be running before the parent is killed"
    );

    // The hard way. No signal handler runs, no destructor runs, nothing gets a chance to clean up. That is
    // the case the containment exists for.
    parent.kill().expect("the helper must be killable");
    parent.wait().expect("reaping the helper");

    // Asked without establishing anything. Establishing here would put this test process into the same
    // kill-on-close job and the guard's drop would terminate the runner, which is what happened the first
    // time and is why this accessor exists.
    let strength = runtrol_childproc::Containment::platform_strength();

    let died = wait_until_gone(grandchild);

    if strength.survives_an_unclean_kill() {
        // A documented platform limitation, not a defect. Reported rather than asserted either way: asserting
        // that it survives would fail the day somebody adds the orphan sweep, and asserting that it dies would
        // fail today for a reason that is written down.
        println!(
            "this platform promises only clean-shutdown containment. grandchild gone: {died}. \
             the gap is closed by an orphan sweep at the next startup, not from inside a killed process"
        );
        return;
    }

    assert!(
        died,
        "the grandchild survived an unclean kill of its parent, on a platform whose containment is supposed \
         to be a kernel guarantee. an agent is now writing to the operator's files with nobody watching"
    );
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
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output();
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
    let output = Command::new("ps").args(["-p", &pid.to_string()]).output();
    match output {
        Ok(output) => output.status.success(),
        Err(error) => panic!("cannot ask the operating system about process {pid}: {error}"),
    }
}
