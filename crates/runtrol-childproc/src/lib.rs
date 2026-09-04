//! Starting child processes correctly, and making sure they die with the daemon.
//!
//! runtrol supervises coding CLIs it did not write, on three operating systems, and the operator's disk is
//! on the other side of them. Two things in this crate are load bearing:
//!
//! - [`resolve`] finds the real program behind the launchers a package manager puts in front of it.
//!   Measured against the live installations on this machine: roughly 80 ms and one process saved per
//!   session start, which the operator pays every time they open a session. What resolution does *not*
//!   reach, and why, is written down where it is measured rather than rounded up into a better number.
//! - [`argv`] refuses arguments that must not reach a command line, and explains which one and why.
//!   Deliberately **not** an escaper: measured on this toolchain, the standard library's own mitigation for
//!   CVE-2024-24576 is correct and complete, and replacing audited platform security code with a
//!   hand-rolled version would be the wrong direction on the one surface where being wrong is remote code
//!   execution.
//!
//! - [`contain`] makes a daemon that dies take its children with it, rather than leaving a coding agent
//!   running against the operator's files with nobody watching. What each platform can actually promise
//!   differs, and the type says which one you got instead of implying the strongest.
//!
//! # Why this is a crate
//!
//! [`contain`] needs `unsafe`: a job object on Windows, and two system calls between fork and exec on Unix.
//! The workspace sets `unsafe_code = "forbid"`, and `forbid` cannot be relaxed from inside a module, so the
//! way to allow it only at an audited platform boundary and machine-forbid it everywhere else is a crate with its
//! own lint table.

//! - [`run`] asks a program one question and reads the answer, under a deadline and a byte ceiling. The
//!   long-lived conversation of a session is not this: it is a transport, and it belongs to a driver.
//!
//! - [`footprint`] how much memory a process is really holding, asked from outside. "memory efficient" is a
//!   number with a gate behind it here, and a budget nobody can measure is a budget nobody is held to.
//!
//! - [`handoff`] stops this process's own handles from travelling to what it starts. Measured: without it, a
//!   command that starts a daemon hands that daemon the shell's own pipe, and the shell waits forever.
//!
//! - [`console_window`] prevents background children from creating their own Windows console. On other
//!   platforms it is deliberately a no-op, so every spawn boundary can apply one policy.

pub mod alive;
pub mod argv;
pub mod console_window;
pub mod contain;
pub mod error;
pub mod footprint;
pub mod handoff;
pub mod held;
pub mod local_terminal;
pub mod os_window;
pub mod process_tree;
pub mod pty;
pub mod resolve;
pub mod run;
pub mod shims;
pub mod stall;
pub mod watch;

pub use alive::{alive, matches_process_start};
pub use argv::{MAX_ARGUMENT_LEN, check_all, check_one};
pub use console_window::hide_console_window;
#[cfg(unix)]
pub use contain::bootstrap_if_requested;
pub use contain::sweep_stale_guard_directories;
pub use contain::{ChildGuard, Containment, Strength, TrackedCommand};
pub use error::SpawnError;
pub use footprint::resident_bytes;
pub use handoff::keep_handles_to_ourselves;
pub use held::{holder_of, holder_of_here, write_locked};
pub use local_terminal::{LocalTerminal, LocalTerminalSize};
pub use process_tree::{ProcessTree, ProcessTreeError, process_identity};
pub use pty::{PtyChild, PtySize, PtySpawn};
pub use resolve::{LauncherKept, Program, ProgramKind, resolve};
pub use run::{MAX_OUTPUT_BYTES, Output, capture, capture_in, capture_with_input};
pub use shims::{PROVIDER_SHIM_PATH_ENV, ProviderShim, ShimError, materialize_provider_shims};
pub use stall::arm_stall_backtrace;
pub use watch::watch_directory;
