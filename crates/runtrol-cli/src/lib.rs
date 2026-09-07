//! What a person types, and what they get back.
//!
//! # This crate cannot open the database, and that is enforced rather than promised
//!
//! The command surface depends on the vocabulary, wire, and process primitives for terminal bridging and exact
//! shutdown confirmation. It cannot see storage, Core, or a driver. Runtime operations therefore go through the
//! daemon's public boundary rather than opening its database from the command process.
//!
//! It is also true for a second reason that has nothing to do with discipline: the database takes an exclusive lock, so
//! a second opener is refused. Two ways of being right about the same thing.
//!
//! # Layout
//!
//! Four things happen between a person typing and a person reading, and each is its own file because each can be wrong
//! on its own:
//!
//! - [`words`] what was typed becomes a request, and nothing is guessed
//! - [`link`] a daemon is reached, and started if there is none
//! - [`ask`] one request goes out and the answers come back
//! - [`lines`] an answer becomes the lines a person reads
//!
//! The two ends of that ([`words`] and [`lines`]) touch nothing at all, which is why both are checked here without a
//! daemon, a socket, or a session.

pub mod administration;
pub mod ask;
pub mod bridge;
#[cfg(windows)]
mod completion;
pub mod courier;
pub mod lines;
pub mod link;
pub mod words;

pub use administration::{AdministrationFailure, administer, is_administration};
pub use ask::{Failed, Outcome, ask, request, request_running, stop_running};
pub use bridge::{BridgeFailure, BridgeProvider, bridge, bridge_providers};
pub use courier::{Admission, CourierFailure, courier};
pub use lines::{NOT_NAMED_YET, render};
pub use link::{DAEMON_ARGUMENT, Unreachable, reach, reach_running};
pub use words::{Misunderstood, understand};
