//! The vocabulary a coding-CLI driver programs against.
//!
//! This crate is the only one a driver author outside this repository depends on. Its public surface
//! is a promise, so it is kept small enough to read in one sitting, and it carries no runtime, no
//! database, and no platform bindings.
//!
//! # What lives here
//!
//! Values that cross the seam between runtrol and a provider, and nothing that acts on them:
//!
//! - [`id`] every identifier runtrol mints or relays
//! - [`path`] the one path type that crosses the seam
//! - [`time`] wall clock time, and why the monotonic clock has no type
//! - [`error`] the error taxonomy a driver returns
//! - [`event`] the normalized event vocabulary, and the rule for what runtrol may read
//!
//! # What deliberately does not live here yet
//!
//! The behavioural contract, meaning the traits a driver implements, arrives together with its first
//! implementation. A trait with no implementor is a guess about a shape that the implementor gets to
//! decide, and this repository does not carry guesses forward as debt.

pub mod error;
pub mod event;
pub mod id;
pub mod path;
pub mod time;

pub use error::ProviderError;
pub use event::{
    AgentEvent, ApprovalKind, ApprovalOption, ApprovalRequest, Attached, BlockedOn, CapabilitySet,
    Chunk, Cost, Declarant, DetachReason, Detached, EventBody, FileId, Level, Notice, NoticeCode,
    OfferedOption, Opaque, PermissionOptionKind, RateLimit, ReplaySource, RiskClass, StopReason,
    ToolCallFrame, ToolCallStatus, ToolKind, TurnEvent, Unmapped, Usage, Window, WithdrawnReason,
};
pub use id::{
    ApprovalId, IdError, MessageId, NativeSessionId, OptionId, ProviderId, SessionId, ToolCallId,
    TurnId,
};
pub use path::{AbsPath, PathError};
pub use time::WallMs;
