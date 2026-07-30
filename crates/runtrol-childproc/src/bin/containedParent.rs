//! A parent that establishes containment, starts a long-lived child, and then waits to be killed.
//!
//! Exists so that `tests/containment.rs` can prove the guarantee rather than assert it. The test cannot check
//! containment from inside its own process: proving "children die when the parent is killed without warning"
//! needs a parent that can be killed without warning, and a test harness is not that.
//!
//! Prints the child's process id on the first line of stdout, then blocks. The test reads the id, kills this
//! process the hard way, and checks whether the child is still there.
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
use std::process::{Command, Stdio};
use std::time::Duration;

/// The argument that puts this binary into its sleeping mode.
const SLEEP_MODE: &str = "--sleep-until-killed";

/// Long enough that the test finishes first, short enough that a stray copy cannot linger.
const SLEEP: Duration = Duration::from_mins(1);

fn main() {
    if std::env::args().any(|argument| argument == SLEEP_MODE) {
        std::thread::sleep(SLEEP);
        return;
    }

    let containment = match runtrol_childproc::Containment::establish() {
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

    let mut command = Command::new(own_path);
    command.arg(SLEEP_MODE);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    containment.prepare(&mut command);

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("could not start the child: {error}");
            std::process::exit(4);
        }
    };

    println!("{}", child.id());
    if let Err(error) = std::io::stdout().flush() {
        eprintln!("could not flush the child id: {error}");
        std::process::exit(5);
    }

    // Block until killed. The containment guard stays alive for exactly as long as this process does, which is
    // the property under test.
    std::thread::sleep(SLEEP);

    // Reading the guard here is what keeps it alive to the end of `main`. `drop` would not: on the platform
    // whose containment carries no resource there is no `Drop` to run, so the call says nothing and lints as
    // pointless. Reading it is honest on both, and it stops a future edit from shortening the guard's scope
    // without anybody noticing.
    if containment.strength().survives_an_unclean_kill() {
        eprintln!("this platform's containment does not cover an unclean kill");
    }
}
