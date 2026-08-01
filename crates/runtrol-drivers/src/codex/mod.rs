//! The driver for the CLI whose sessions share one daemon and speak request and response.
//!
//! # What was measured, and when
//!
//! Everything here comes from running version 0.145.0 on this machine and from the protocol schema it
//! generates, rather than from reading about it. The surface it offers (126 methods runtrol may call, 70
//! notifications, 11 questions it asks back), the latencies, and the shape of an ending. See [`bound`] for the
//! list runtrol binds and [`map`] for what it does with each.
//!
//! # The three facts that shape the whole driver
//!
//! **One process serves every session.** The other supported CLI runs a process per session; this one is a
//! daemon that multiplexes conversations over one pair of streams. That is what makes N sessions cost one
//! child, and it is why [`conn`] exists at all.
//!
//! **The provider names the conversation, and runtrol does not.** The identifier arrives in the answer to the
//! call that opens it. The other CLI takes an identifier runtrol issues, and nothing outside each driver
//! depends on which way round it is.
//!
//! **Submitting a turn is not starting one.** Measured: the call that submits answers in two milliseconds
//! with a turn that is in progress and carries no work, and the turn then runs for eight seconds. A probe that
//! read the answer as the result reported the turn as finished instantly. The whole turn vocabulary
//! distinguishes the receipt from the beginning from the ending because of this.

pub mod agent;
mod approval;
pub mod bound;
pub mod conn;
pub mod map;
pub mod provider;

pub use agent::CodexAgent;
pub use bound::{Answer, BoundCall, BoundNotice, BoundRequest, CALLS, NOTICES, REQUESTS, TERMINAL};
pub use conn::{Connection, Delivery, INBOX_DEPTH, Inbox};
pub use map::{Ended, Frame, MapError};
pub use provider::CodexProvider;
