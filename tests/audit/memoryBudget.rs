//! Gate: the daemon's memory budget is a number, measured, and red when it is exceeded.
//!
//! "Memory efficient" is the claim this product is built around: it sits resident on somebody's machine all day
//! beside the CLIs it supervises, and the reason it is allowed to is that it costs almost nothing to leave
//! running. A claim like that is worth exactly as much as the gate behind it. Without one, the release that
//! doubles it is discovered by an operator rather than by this file.
//!
//! # What is measured, and against what
//!
//! An idle daemon: started, listening, serving nothing. That is the state it is in for almost all of its life, so
//! it is the state the number is about. Measured from outside by asking the operating system, because a process
//! reporting on itself is a process reporting what its allocator believes rather than what it holds.
//!
//! # The numbers, and what they were set from
//!
//! Measured on this machine, an idle daemon with nothing running:
//!
//! | build | held |
//! |---|---:|
//! | release | 11,448,320 bytes (10.9 MiB) |
//! | debug | 12,406,784 bytes (11.8 MiB) |
//!
//! The budgets below sit above those with room for a machine under load and for the difference between platforms,
//! and not much more. The first version of this file allowed eight times the measured number, which is not a
//! budget: a limit nothing can reach is a limit that never says anything, and it would have let the footprint
//! quadruple in silence.
//!
//! The two builds differ by less than a mebibyte, which is itself the useful fact: almost all of this is the
//! runtime and the allocator rather than runtrol's own code, so the number to watch is what a session adds.
//!
//! Each build still gets its own, because an optimized one is a different program and holding both to one number
//! means either a debug budget nobody can meet or a release budget that permits anything.
//!
//! # What is not here yet
//!
//! The per-session increment. Measuring it means starting a real session, which means a real CLI, which is not
//! on every machine this runs on. It arrives with the session smoke gate that already has to solve that, and
//! saying so here is better than a number measured against nothing.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What an idle daemon may hold in a release build, in bytes.
///
/// The number an operator is entitled to. Raising it is not a code change somebody makes on the way past: it is
/// the product getting more expensive to leave running, and it needs the person who decided that to have decided
/// it on purpose. Measured at 11,448,320 bytes.
const RELEASE_BUDGET: u64 = 16 * 1024 * 1024;

/// What an idle daemon may hold in a debug build, in bytes.
///
/// Larger for a reason that has nothing to do with the design: an unoptimized build with debug information is a
/// different program. This exists so the gate runs on the build a developer already has, rather than being a gate
/// that only ever runs somewhere else. Measured at 12,406,784 bytes.
const DEBUG_BUDGET: u64 = 20 * 1024 * 1024;

/// How long to let a daemon settle before asking what it holds.
///
/// It establishes containment, reads its manifests and binds an endpoint. Measuring during that is measuring the
/// start rather than the resting state.
const SETTLE: Duration = Duration::from_secs(5);

/// How long to wait for the endpoint to appear before giving up on the daemon.
const START_WITHIN: Duration = Duration::from_secs(20);

/// A daemon started for the length of one measurement, stopped whatever happens.
struct Idle {
    child: Child,
    home: PathBuf,
}

impl Drop for Idle {
    fn drop(&mut self) {
        // Killed rather than asked politely: this is a measurement, not a session, and leaving a daemon behind
        // would have the next run measure the wrong process.
        let stopped = self.child.kill();
        drop(stopped);
        let waited = self.child.wait();
        drop(waited);
        let removed = std::fs::remove_dir_all(&self.home);
        drop(removed);
    }
}

/// The runtrol binary next to this test, and whether it was built for release.
///
/// Found by walking up from the test executable rather than by an environment variable, because the variable
/// cargo sets for this names binaries of this package and runtrol is not one of them.
fn the_binary() -> Option<(PathBuf, bool)> {
    let Ok(here) = std::env::current_exe() else {
        return None;
    };
    // `target/<profile>/deps/<test>` is where a test executable lives, so the binary is two directories up.
    let profile_dir = here.parent()?.parent()?;
    let name = if cfg!(windows) {
        "runtrol.exe"
    } else {
        "runtrol"
    };
    let binary = profile_dir.join(name);
    if !binary.is_file() {
        return None;
    }
    let release = profile_dir
        .file_name()
        .is_some_and(|profile| profile == "release");
    Some((binary, release))
}

/// Start an idle daemon in a home of its own.
fn start(binary: &Path, home: &Path) -> Option<Idle> {
    let mut command = Command::new(binary);
    command
        .arg("daemon")
        .env("RUNTROL_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    runtrol_childproc::hide_console_window(&mut command);
    let started = command.spawn();
    let Ok(child) = started else {
        return None;
    };

    let idle = Idle {
        child,
        home: home.to_path_buf(),
    };

    // Wait for it to have opened its home, which is the first thing it does and the first thing that can fail.
    // The directories it creates there are what say so, and they are the same on both platforms.
    let give_up_at = Instant::now() + START_WITHIN;
    let mut opened = false;
    while Instant::now() < give_up_at {
        if home.join("providers").is_dir() {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if !opened {
        return None;
    }

    // Then let it settle, and take still being alive as having started. Every way this daemon can fail to start
    // ends the process, so a daemon that is still running after settling is a daemon that is serving.
    std::thread::sleep(SETTLE);
    let mut idle = idle;
    match idle.child.try_wait() {
        Ok(None) => Some(idle),
        // It exited, or it cannot be asked. Either way there is nothing running to measure, and reporting a
        // number for it would be reporting one for a process that is not there.
        Ok(Some(_)) | Err(_) => None,
    }
}

#[test]
fn an_idle_daemon_stays_inside_its_budget() {
    let Some((binary, release)) = the_binary() else {
        // Said out loud. A gate that skipped quietly would be a gate that reports green on a machine where it
        // never ran, which is worse than not having it.
        panic!(
            "the runtrol binary is not built next to this test. run \
             `cargo build -p runtrol --bin runtrol` before the audit test"
        );
    };

    let home = std::env::temp_dir().join(format!("runtrol-budget-{}", std::process::id()));
    let removed = std::fs::remove_dir_all(&home);
    drop(removed);

    let Some(idle) = start(&binary, &home) else {
        panic!(
            "the daemon did not start within {} seconds",
            START_WITHIN.as_secs()
        );
    };

    let held = runtrol_childproc::resident_bytes(idle.child.id())
        .expect("a daemon that is running can be asked what it holds");

    let (budget, which) = if release {
        (RELEASE_BUDGET, "release")
    } else {
        (DEBUG_BUDGET, "debug")
    };

    assert!(
        held <= budget,
        "an idle daemon held {held} bytes, and the {which} budget is {budget}. \
         raising the budget is a decision about what runtrol costs to leave running, not an edit on the way past"
    );

    // And a measurement that answered nothing would pass this gate forever.
    assert!(
        held > 1024 * 1024,
        "an idle daemon holding {held} bytes is not a measurement"
    );
}
