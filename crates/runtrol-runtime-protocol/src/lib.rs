//! Public, provider-neutral Runtime protocol.
//!
//! This crate owns the negotiated wire vocabulary. It deliberately knows nothing about Core, provider drivers,
//! storage, daemon composition, or first-party product surfaces.

mod error;
mod inventory;
mod method;
mod revision;
mod rpc;
mod schema;

pub use error::{RuntimeError, RuntimeErrorKind};
pub use inventory::{
    InstallationObservation, InstallationState, LifecycleState, ManagedSessionList,
    ProviderDescriptor, ProviderId, ProviderList, RuntimeSessionId, SessionDescriptor,
};
pub use method::RuntimeMethod;
pub use revision::{
    FINALIZED_REVISIONS, ProtocolRevision, REVISION_2026_08_13, RevisionError, negotiate,
};
pub use rpc::{
    ClientCapabilities, ClientInfo, ErrorResponse, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RuntimeCapabilities, RuntimeInstance,
    RuntimeLimits, SuccessResponse,
};
pub use schema::{PUBLIC_SCHEMA_NAME, public_schema};

/// Maximum JSON payload bytes in one public Runtime frame.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024 + 64 * 1024;

/// Maximum caller input bytes admitted by the public contract.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// Maximum items returned in one catalogue page.
pub const MAX_PAGE_ITEMS: u16 = 100;

/// Maximum simultaneous subscriptions exposed to one client connection.
pub const MAX_SUBSCRIPTIONS: u16 = 32;
