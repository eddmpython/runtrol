//! Public, provider-neutral Runtime protocol.
//!
//! This crate owns the negotiated wire vocabulary. It deliberately knows nothing about Core, provider drivers,
//! storage, daemon composition, or first-party product surfaces.

mod approval;
mod capability;
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
mod session_open;
mod terminal;
mod windows;

pub use approval::{
    ListPendingApprovalsParams, PendingApproval, PendingApprovalList, RespondApprovalParams,
    RuntimeApprovalKind, RuntimeApprovalOption, RuntimeApprovalOptionKind, RuntimeApprovalRisk,
};
pub use capability::{
    CapabilityFreshness, GetProviderCapabilitiesParams, ProviderCapabilityAvailability,
    ProviderCapabilityObservation, ProviderCapabilityProvenance, RuntimeProviderCapabilities,
};
pub use error::{RuntimeError, RuntimeErrorKind};
pub use integration::{
    AppScope, EnrollmentDecision, EnrollmentManifest, EnrollmentReceipt, IntegrationAuthentication,
    IntegrationGrant, IntegrationId, PendingEnrollmentId, RequestEnrollmentParams,
    RotateIntegrationKeyParams, ServerChallenge, UnknownAppScope, WatchEnrollmentParams,
    enrollment_signing_payload, initialization_signing_payload, key_rotation_signing_payload,
    self_approval_signing_payload,
};
pub use inventory::{
    GetSessionParams, InstallationObservation, InstallationState, LifecycleState,
    ManagedSessionList, ProviderAccount, ProviderAccountStatus, ProviderDescriptor, ProviderHelp,
    ProviderId, ProviderLimitsAbsent, ProviderLimitsAbsentKind, ProviderList, ProviderUsageCost,
    ProviderUsageGauge, ProviderUsageList, ProviderUsageWindow, ProviderWatchEndReason,
    ProviderWatchEndedNotification, ProvidersChangedNotification,
    ProvidersUsageChangedNotification, RuntimeSessionId, SessionDescriptor,
    SessionIndexChangedNotification, SessionIndexEndReason, SessionIndexEndedNotification,
    WaitingOn, WatchProvidersParams, WatchProvidersResult, WatchSessionIndexParams,
    WatchSessionIndexResult,
};
pub use locator::{
    MAX_RUNTIME_GENERATIONS, RUNTIME_LOCATOR_SCHEMA, RuntimeEndpointKind, RuntimeGeneration,
    RuntimeLocatorRecord,
};
pub use method::RuntimeMethod;
pub use models::{
    ListModelsParams, RuntimeModelCatalog, RuntimeModelChoice, RuntimeReasoningChoice,
};
pub use native_sessions::{
    CatalogueCoverage, CatalogueSource, ListNativeSessionsParams, MAX_NATIVE_ADOPTION_TOKEN_BYTES,
    MAX_NATIVE_PUBLIC_CURSOR_BYTES, NATIVE_CURSOR_LIFETIME_MS, NativeActivity,
    NativeActivityParams, NativeResumeCapability, NativeSessionCatalogue, NativeSessionDescriptor,
};
pub use revision::{
    FINALIZED_REVISIONS, ProtocolRevision, REVISION_2026_08_13, REVISION_2026_08_27, RevisionError,
    negotiate,
};
pub use rpc::{
    ClientCapabilities, ClientInfo, ErrorResponse, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RuntimeCapabilities, RuntimeInstance,
    RuntimeLimits, SuccessResponse,
};
pub use schema::{PUBLIC_SCHEMA_NAME, public_schema};
pub use session::{
    AcquireControlParams, CONTROL_LEASE_LIFETIME_MS, ControlLease, ControlLeaseParams,
    CoolSessionParams, EventCursor, EventGap, ForgetSessionParams, IDEMPOTENCY_WINDOW_MS,
    InterruptParams, LaggedNotification, MAX_ATTACHMENT_BASE64_BYTES, MAX_IDEMPOTENCY_RECORDS,
    MAX_INPUT_BLOCKS, MAX_INPUT_IMAGES, MUTATION_CLOCK_SKEW_MS, MutationRequestId,
    MutationRequestIdError, PublicInputBlock, RuntimeEventNotification, SetModeParams,
    SetModelParams, SubmitBlocksParams, SubmitInputParams, WatchEventsParams, WatchEventsResult,
};
pub use session_open::{
    AdoptNativeSessionParams, ArchiveNativeSessionParams, DeleteNativeSessionParams,
    MAX_MODEL_SELECTION_BYTES, MAX_PERMISSION_SELECTION_BYTES, MAX_REASONING_SELECTION_BYTES,
    ResumeSessionParams, SessionOpenResult, SessionWorkspaceAccess, StartSessionParams,
};
pub use terminal::{
    ListTerminalsParams, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_INDEX_ITEMS, MAX_TERMINAL_OUTPUT_BYTES,
    MAX_TERMINAL_ROWS, MAX_TERMINAL_SCREEN_BYTES, MAX_TERMINAL_VIEW_QUEUE_CHUNKS,
    MAX_TERMINAL_WRITE_BYTES, RuntimeTerminalId, RuntimeTerminalViewId,
    TerminalAcquireControlParams, TerminalAttachParams, TerminalControlLease,
    TerminalControlParams, TerminalDescriptor, TerminalDetachParams, TerminalExitedNotification,
    TerminalGeometry, TerminalIdError, TerminalIndexChangedNotification, TerminalIndexEndReason,
    TerminalIndexEndedNotification, TerminalIndexSnapshot, TerminalLaggedNotification,
    TerminalOpenParams, TerminalOpenTarget, TerminalOutputNotification, TerminalProcessState,
    TerminalResizeParams, TerminalStopParams, TerminalViewOpened, TerminalWriteParams,
    WatchTerminalIndexParams, WatchTerminalIndexResult,
};
pub use windows::{
    ListWindowsParams, MAX_OBSERVED_TERMINALS, MAX_REGISTERED_WINDOWS, MAX_WINDOW_FOLDERS,
    MAX_WINDOW_TEXT_CHARS, ObservedCommand, ObservedTerminal, WatchWindowIndexParams,
    WatchWindowIndexResult, WindowDescriptor, WindowIndexChangedNotification, WindowIndexEndReason,
    WindowIndexEndedNotification, WindowIndexSnapshot, WindowRegisterParams, WindowRegistration,
    WindowUpdateParams,
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
