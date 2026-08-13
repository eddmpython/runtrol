//! Public, provider-neutral Runtime protocol.
//!
//! This crate owns the negotiated wire vocabulary. It deliberately knows nothing about Core, provider drivers,
//! storage, daemon composition, or first-party product surfaces.

mod error;
mod integration;
mod inventory;
mod locator;
mod method;
mod models;
mod native_sessions;
mod revision;
mod rpc;
mod schema;
mod session;

pub use error::{RuntimeError, RuntimeErrorKind};
pub use integration::{
    AppScope, EnrollmentDecision, EnrollmentManifest, EnrollmentReceipt, IntegrationAuthentication,
    IntegrationGrant, IntegrationId, PendingEnrollmentId, RequestEnrollmentParams, ServerChallenge,
    UnknownAppScope, WatchEnrollmentParams, enrollment_signing_payload,
    initialization_signing_payload,
};
pub use inventory::{
    InstallationObservation, InstallationState, LifecycleState, ManagedSessionList,
    ProviderDescriptor, ProviderId, ProviderList, RuntimeSessionId, SessionDescriptor,
};
pub use locator::{RUNTIME_LOCATOR_SCHEMA, RuntimeEndpointKind, RuntimeLocatorRecord};
pub use method::RuntimeMethod;
pub use models::{
    ListModelsParams, RuntimeModelCatalog, RuntimeModelChoice, RuntimeReasoningChoice,
};
pub use native_sessions::{
    CatalogueCoverage, CatalogueSource, ListNativeSessionsParams, MAX_NATIVE_PUBLIC_CURSOR_BYTES,
    NATIVE_CURSOR_LIFETIME_MS, NativeResumeCapability, NativeSessionCatalogue,
    NativeSessionDescriptor,
};
pub use revision::{
    FINALIZED_REVISIONS, ProtocolRevision, REVISION_2026_08_13, RevisionError, negotiate,
};
pub use rpc::{
    ClientCapabilities, ClientInfo, ErrorResponse, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RuntimeCapabilities, RuntimeInstance,
    RuntimeLimits, SuccessResponse,
};
pub use schema::{PUBLIC_SCHEMA_NAME, public_schema};
pub use session::{
    AcquireControlParams, CONTROL_LEASE_LIFETIME_MS, ControlLease, ControlLeaseParams, EventCursor,
    EventGap, IDEMPOTENCY_WINDOW_MS, InterruptParams, LaggedNotification, MAX_IDEMPOTENCY_RECORDS,
    MUTATION_CLOCK_SKEW_MS, MutationRequestId, MutationRequestIdError, RuntimeEventNotification,
    SubmitInputParams, WatchEventsParams, WatchEventsResult,
};

/// Maximum JSON payload bytes in one public Runtime frame.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024 + 64 * 1024;

/// Maximum caller input bytes admitted by the public contract.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

/// Maximum items returned in one catalogue page.
pub const MAX_PAGE_ITEMS: u16 = 100;

/// Maximum simultaneous subscriptions exposed to one client connection.
pub const MAX_SUBSCRIPTIONS: u16 = 32;

/// Lifetime of one server-first connection challenge.
pub const CHALLENGE_LIFETIME_MS: u64 = 60_000;

/// Lifetime of one local integration enrollment decision.
pub const ENROLLMENT_LIFETIME_MS: u64 = 10 * 60_000;

/// Maximum active pending integration enrollments in one Runtime home.
pub const MAX_PENDING_ENROLLMENTS: u16 = 64;

/// Maximum finalized protocol revisions one client may offer during initialization.
pub const MAX_REVISION_OFFERS: u16 = 16;
