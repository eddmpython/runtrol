//! Public Rust client for one shared, per-user Runtrol Runtime.
//!
//! The client locates and validates Runtime, negotiates a finalized public revision, and exposes only typed public
//! operations. Dropping it closes one connection and never stops Runtime or a provider session.

mod client;
mod connection;
mod identity;
mod locator;

pub use client::{
    ApprovalClient, ClientOptions, EnrollmentProposal, EventSubscription, IntegrationClient,
    ProviderClient, RuntimeClient, SessionClient, SessionIndexNotification,
    SessionIndexSubscription, SessionNotification,
};
pub use identity::{IntegrationCredentials, IntegrationIdentity};
pub use locator::{LocatorError, LocatorState, RuntimeLocator, ValidatedLocator};
pub use runtrol_runtime_protocol as protocol;

/// A public client operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// Runtime could not be located safely.
    #[error(transparent)]
    Locator(#[from] LocatorError),
    /// The local byte stream could not complete an operation.
    #[error("Runtime transport failed while {doing}: {detail}")]
    Transport {
        /// The bounded operation.
        doing: &'static str,
        /// Operating-system error text.
        detail: String,
    },
    /// A public frame or JSON-RPC envelope violated the selected revision.
    #[error("Runtime protocol violation: {0}")]
    Protocol(String),
    /// Runtime returned a stable machine failure.
    #[error("Runtime refused the request: {0}")]
    Runtime(#[from] runtrol_runtime_protocol::RuntimeError),
}
