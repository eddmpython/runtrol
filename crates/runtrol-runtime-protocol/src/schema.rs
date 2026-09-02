//! Deterministic JSON schema derived from the Rust public DTO source of truth.

use schemars::{JsonSchema, schema_for};

use crate::{
    AcquireControlParams, AdoptNativeSessionParams, ArchiveNativeSessionParams, ControlLease,
    ControlLeaseParams, CoolSessionParams, DeleteNativeSessionParams, EnrollmentDecision,
    EnrollmentReceipt, ForgetSessionParams, GetProviderCapabilitiesParams, GetSessionParams,
    InitializeParams, InitializeResult, IntegrationGrant, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LaggedNotification, ListModelsParams, ListNativeSessionsParams,
    ListPendingApprovalsParams, ListTerminalsParams, ListWindowsParams, ManagedSessionList,
    NativeActivity, NativeActivityParams, NativeSessionCatalogue, ObservedCommand,
    ObservedTerminal, PendingApprovalList, ProviderList, ProviderUsageList,
    ProviderWatchEndedNotification, ProvidersChangedNotification,
    ProvidersUsageChangedNotification, RequestEnrollmentParams, RespondApprovalParams,
    ResumeSessionParams, RotateIntegrationKeyParams, RuntimeEventNotification,
    RuntimeLocatorRecord, RuntimeMethod, RuntimeModelCatalog, RuntimeProviderCapabilities,
    RuntimeTerminalId, RuntimeTerminalViewId, ServerChallenge, SessionDescriptor,
    SessionIndexChangedNotification, SessionIndexEndedNotification, SessionOpenResult,
    SetModeParams, SetModelParams, StartSessionParams, SubmitBlocksParams, SubmitInputParams,
    TerminalAcquireControlParams, TerminalAttachParams, TerminalControlLease,
    TerminalControlParams, TerminalDescriptor, TerminalDetachParams, TerminalExitedNotification,
    TerminalIndexChangedNotification, TerminalIndexEndedNotification, TerminalIndexSnapshot,
    TerminalLaggedNotification, TerminalOpenParams, TerminalOutputNotification,
    TerminalResizeParams, TerminalStopParams, TerminalViewOpened, TerminalWriteParams,
    WatchEnrollmentParams, WatchEventsParams, WatchEventsResult, WatchProvidersParams,
    WatchProvidersResult, WatchSessionIndexParams, WatchSessionIndexResult,
    WatchTerminalIndexParams, WatchTerminalIndexResult, WatchWindowIndexParams,
    WatchWindowIndexResult, WindowDescriptor, WindowIndexChangedNotification,
    WindowIndexEndedNotification, WindowIndexSnapshot, WindowRegisterParams, WindowRegistration,
    WindowUpdateParams,
};

/// Checked schema filename inside this package.
pub const PUBLIC_SCHEMA_NAME: &str = "runtime.schema.json";

/// Root containing every type admitted by the initial public boundary.
#[derive(JsonSchema)]
#[allow(
    dead_code,
    reason = "fields exist only to make every public definition reachable from one generated schema"
)]
struct PublicProtocolSchema {
    method: RuntimeMethod,
    request: JsonRpcRequest,
    notification: JsonRpcNotification,
    response: JsonRpcResponse,
    initialize_params: InitializeParams,
    initialize_result: InitializeResult,
    challenge: ServerChallenge,
    request_enrollment: RequestEnrollmentParams,
    enrollment_receipt: EnrollmentReceipt,
    watch_enrollment: WatchEnrollmentParams,
    enrollment_decision: EnrollmentDecision,
    integration_grant: IntegrationGrant,
    rotate_integration_key: RotateIntegrationKeyParams,
    runtime_locator: RuntimeLocatorRecord,
    provider_list: ProviderList,
    provider_usage_list: ProviderUsageList,
    get_provider_capabilities: GetProviderCapabilitiesParams,
    provider_capabilities: RuntimeProviderCapabilities,
    list_models: ListModelsParams,
    model_catalogue: RuntimeModelCatalog,
    list_native_sessions: ListNativeSessionsParams,
    native_session_catalogue: NativeSessionCatalogue,
    native_activity_params: NativeActivityParams,
    native_activity: NativeActivity,
    managed_session_list: ManagedSessionList,
    get_session: GetSessionParams,
    session_descriptor: SessionDescriptor,
    start_session: StartSessionParams,
    adopt_native_session: AdoptNativeSessionParams,
    resume_session: ResumeSessionParams,
    session_open_result: SessionOpenResult,
    acquire_control: AcquireControlParams,
    control_lease: ControlLease,
    control_lease_params: ControlLeaseParams,
    cool_session: CoolSessionParams,
    forget_session: ForgetSessionParams,
    delete_native_session: DeleteNativeSessionParams,
    archive_native_session: ArchiveNativeSessionParams,
    submit_input: SubmitInputParams,
    submit_blocks: SubmitBlocksParams,
    set_model: SetModelParams,
    set_mode: SetModeParams,
    watch_events: WatchEventsParams,
    watch_events_result: WatchEventsResult,
    runtime_event: RuntimeEventNotification,
    lagged: LaggedNotification,
    list_pending_approvals: ListPendingApprovalsParams,
    pending_approvals: PendingApprovalList,
    respond_approval: RespondApprovalParams,
    watch_session_index: WatchSessionIndexParams,
    watch_session_index_result: WatchSessionIndexResult,
    session_index_changed: SessionIndexChangedNotification,
    session_index_ended: SessionIndexEndedNotification,
    watch_providers: WatchProvidersParams,
    watch_providers_result: WatchProvidersResult,
    providers_changed: ProvidersChangedNotification,
    providers_usage_changed: ProvidersUsageChangedNotification,
    provider_watch_ended: ProviderWatchEndedNotification,
    runtime_terminal_id: RuntimeTerminalId,
    runtime_terminal_view_id: RuntimeTerminalViewId,
    list_terminals: ListTerminalsParams,
    terminal_index: TerminalIndexSnapshot,
    terminal_descriptor: TerminalDescriptor,
    watch_terminal_index: WatchTerminalIndexParams,
    watch_terminal_index_result: WatchTerminalIndexResult,
    terminal_open: TerminalOpenParams,
    terminal_attach: TerminalAttachParams,
    terminal_view_opened: TerminalViewOpened,
    terminal_control_lease: TerminalControlLease,
    terminal_acquire_control: TerminalAcquireControlParams,
    terminal_control: TerminalControlParams,
    terminal_write: TerminalWriteParams,
    terminal_resize: TerminalResizeParams,
    terminal_detach: TerminalDetachParams,
    terminal_stop: TerminalStopParams,
    terminal_index_changed: TerminalIndexChangedNotification,
    terminal_index_ended: TerminalIndexEndedNotification,
    terminal_output: TerminalOutputNotification,
    terminal_lagged: TerminalLaggedNotification,
    terminal_exited: TerminalExitedNotification,
    window_register: WindowRegisterParams,
    window_registration: WindowRegistration,
    window_update: WindowUpdateParams,
    observed_terminal: ObservedTerminal,
    observed_command: ObservedCommand,
    window_descriptor: WindowDescriptor,
    list_windows: ListWindowsParams,
    window_index: WindowIndexSnapshot,
    watch_window_index: WatchWindowIndexParams,
    watch_window_index_result: WatchWindowIndexResult,
    window_index_changed: WindowIndexChangedNotification,
    window_index_ended: WindowIndexEndedNotification,
}

/// Generate the language-neutral public schema from the Rust DTOs.
///
/// # Errors
///
/// A serialization failure from `serde_json`. The schema graph contains no fallible user data, so such a failure is a
/// release tooling defect rather than a Runtime request failure.
pub fn public_schema() -> Result<serde_json::Value, serde_json::Error> {
    let mut schema = serde_json::to_value(schema_for!(PublicProtocolSchema))?;
    if let Some(root) = schema.as_object_mut() {
        root.insert(
            "x-runtrol-finalized-revisions".to_owned(),
            serde_json::to_value(crate::FINALIZED_REVISIONS)?,
        );
        root.insert(
            "x-runtrol-limits".to_owned(),
            serde_json::to_value(crate::RuntimeLimits::default())?,
        );
    }
    Ok(schema)
}
