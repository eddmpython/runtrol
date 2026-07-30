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
//! - [`wire`] what the command surface asks and what the daemon answers
//! - [`transport`] getting those frames between two processes on one machine

pub mod frame;
pub mod transport;
pub mod wire;

pub use frame::{Decoded, FrameError, MAX_FRAME, WIRE_VERSION, check_version, decode, encode};
pub use transport::{Connection, Listener, TransportError, connect};
pub use wire::{ProviderLine, Request, Response, SessionLine, WireError, agree};
