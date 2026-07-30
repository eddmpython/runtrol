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
//! # What arrives next
//!
//! Containment: a job object on Windows and a process group elsewhere, so that a daemon which dies takes
//! its children with it rather than leaving a coding agent running against the operator's files. That is
//! the part of this crate that needs `unsafe`, and it lands with its own lint override rather than one
//! written ahead of the code that needs it.

pub mod argv;
pub mod error;
pub mod resolve;

pub use argv::{MAX_ARGUMENT_LEN, check_all, check_one};
pub use error::SpawnError;
pub use resolve::{LauncherKept, Program, ProgramKind, resolve};
