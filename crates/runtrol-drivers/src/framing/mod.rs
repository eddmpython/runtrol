//! Getting bytes to and from a child, in the shapes these CLIs actually speak.
//!
//! # Three framings, and why they are not one
//!
//! - [`ndjson`] one JSON object per line, which is what one of these CLIs speaks.
//! - JSON-RPC over the same stream, which is what the other one speaks. A different protocol with its own
//!   identifier policy and its own bounds, so a different file.
//! - runtrol's own frames between its own processes, which live in the crate that owns that wire.
//!
//! Sharing them would produce exactly the module that takes anything and belongs to nobody. They have one
//! thing in common, which is that bytes arrive in pieces, and that is not enough to justify an abstraction.
//!
//! # No terminal emulation, and that is a correctness decision
//!
//! Measured: the platform's console layer hard-wraps output at the terminal width, which splits one long line
//! of JSON into several. A framing that read those would assemble two invalid documents out of one valid one.
//! So a child's output is read as a pipe and never through a console, and the design is arranged so nothing
//! ever wants a terminal.

pub mod jsonrpc;
pub mod ndjson;

pub use jsonrpc::{FrameError, Incoming, Pending, RequestId, WireError};
pub use ndjson::{LineError, Lines, MAX_LINE, READ_BUFFER};
