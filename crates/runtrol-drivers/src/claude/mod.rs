//! The driver for the CLI that runs one process per session and speaks a stream of objects.
//!
//! # What was measured, and when
//!
//! Everything here comes from running version 2.1.220 on this machine rather than from reading about it. The
//! frames it actually emits, the flag that exists and is not in its own help, and the terminal frame that is not
//! the one the design note recorded. See [`bound`] for the table and [`map`] for what runtrol does with each.
//!
//! # The two facts that shape the whole driver
//!
//! **runtrol issues the identifier.** Measured: the identifier handed to `--session-id` comes back unchanged in
//! the startup frame and in the terminal frame. So this provider's identifier and runtrol's own are the same
//! value, which is why deleting everything runtrol stores loses nothing: the session is still there under a name
//! the CLI itself knows.
//!
//! **The CLI declares its own capabilities.** The startup frame carries a capability list, its own version, its
//! tools, its skills and its agents. Nothing here infers a capability from a version string, because nothing
//! has to.

pub mod bound;
pub mod map;

pub use bound::{BoundFlag, BoundFrame, CONTROL, FLAGS, FRAMES, TERMINAL};
pub use map::{Ended, Frame, MapError, Startup};
