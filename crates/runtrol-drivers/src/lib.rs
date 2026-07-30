//! The built-in drivers. The kind table lives here, and so does every provider proper noun.
//!
//! This is the only place in `crates/` where a CLI may be named. The kernel selects code by `kind` and has
//! nowhere to write a name, which is what makes "adding a provider does not touch the kernel" a fact a gate can
//! check.
//!
//! # Layout
//!
//! - [`framing`] getting bytes to and from a child in the shapes these CLIs speak

pub mod framing;

pub use framing::{FrameError, Incoming, LineError, Lines, Pending, RequestId};

// This edge is declared in this crate's manifest and enforced by `tests/audit/dependencyDirection.rs`. Until
// the module that starts a child arrives, this line is what makes the declaration real: `cargo shear` reports a
// dependency nothing names.
use runtrol_childproc as _;
