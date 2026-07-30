//! The frames runtrol speaks to itself, and both ends of the local connection.
//!
//! # Why this is its own crate
//!
//! Two reasons, and both are enforcement rather than tidiness.
//!
//! The wire needs its own compatibility story. The launcher updates one file at a time, so a newer command surface
//! will meet an older daemon on a real machine, and that is the ordinary consequence of updating rather than an edge
//! case.
//!
//! And the command surface must be able to speak it **without linking storage, drivers, or the kernel**. As a crate
//! whose dependency list is the vocabulary and nothing else, "the command surface never opens the database" is a fact
//! the compiler holds rather than a comment somebody has to keep true.
//!
//! # Layout
//!
//! - [`frame`] the length-prefixed frame, its bound, and the version both sides check

pub mod frame;

pub use frame::{Decoded, FrameError, MAX_FRAME, WIRE_VERSION, check_version, decode, encode};

// The vocabulary is this crate's declared dependency and enforced by `tests/audit/dependencyDirection.rs`. The framing
// below is bytes and needs none of it; the request and response types that do arrive in their own step. Until then this
// line is what makes the declaration real, because `cargo shear` reports a dependency nothing names.
use runtrol_provider as _;
