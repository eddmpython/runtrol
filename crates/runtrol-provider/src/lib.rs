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
//! - [`manifest`] what a provider declares about itself, and the rule that keeps it small
//! - [`command`] what a driver is told to do, and what it hands back
//! - [`agent`] the two traits a driver implements
//!
//! # The behavioural contract arrived with its first implementation
//!
//! [`agent`] was deliberately absent until a driver existed to implement it. A trait with no implementor is a
//! guess about a shape the implementor gets to decide, and the guess would have been wrong: the recorded design
//! said a turn ends on one frame and the CLI ends it on another, which is exactly the kind of thing only running
//! it tells you.

pub mod account;
pub mod agent;
pub mod capability;
pub mod catalog;
pub mod command;
pub mod error;
pub mod event;
pub mod id;
pub mod manifest;
pub mod native_catalogue;
pub mod path;
pub mod time;

pub use account::{
    AccountLimits, AccountReport, AccountStatus, MAX_ACCOUNT_TOKEN_BYTES, account_token,
};
pub use agent::{Agent, Provider};
pub use capability::{
    ProviderCapabilities, ProviderCapability, ProviderCapabilitySource, ProviderCapabilityState,
};
pub use catalog::{
    MAX_MODEL_CHOICES, MAX_REASONING_CHOICES, ModelCatalog, ModelChoice, ReasoningChoice,
};
pub use command::{
    AgentCommand, CloseMode, ContentBlock, DEFAULT_GRACE_MS, Disposition, OpenIntent, Produced,
};
pub use error::ProviderError;
pub use event::{
    AgentEvent, ApprovalKind, ApprovalOption, ApprovalRequest, Attached, BlockedOn, CapabilitySet,
    Chunk, Cost, Declarant, DetachReason, Detached, EventBody, Level, Notice, NoticeCode,
    OfferedOption, Opaque, PermissionOptionKind, RateLimit, RiskClass, StopReason, ToolCallFrame,
    ToolCallStatus, ToolKind, TurnEvent, Unmapped, Usage, WatchCursor, WatchGap, Window,
    WithdrawnReason,
};
pub use id::{
    ApprovalId, IdError, MessageId, NativeSessionId, OptionId, ProviderId, SessionId, StreamId,
    TerminalId, ToolCallId, TurnId,
};
pub use manifest::{
    AccountSpec, BinSpec, EventsSpec, FallbackSpec, FlagProbe, HelpCommands, Kind, MANIFEST_SCHEMA,
    Manifest, ManifestError, ModelAliases, ProbeSpec, SecretPaths, StoreSpec, TransportSpec,
    TuiSpec, UpdateHint, UpdateSpec, VersionParse, VersionProbe,
};
pub use native_catalogue::{
    MAX_NATIVE_ADDITIONAL_DIRECTORIES, MAX_NATIVE_CURSOR_BYTES, MAX_NATIVE_SESSION_ITEMS,
    MAX_NATIVE_TIMESTAMP_BYTES, MAX_NATIVE_TITLE_BYTES, NativeCatalogueCoverage,
    NativeCatalogueSource, NativeResumeCapability, NativeSessionArchival, NativeSessionCatalogue,
    NativeSessionDeletion, NativeSessionEntry, NativeSessionQuery,
};
pub use path::{AbsPath, PathError, WorkspaceAccess};
pub use time::WallMs;
