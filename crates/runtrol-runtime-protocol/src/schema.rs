//! Deterministic JSON schema derived from the Rust public DTO source of truth.

use schemars::{JsonSchema, schema_for};

use crate::{
    AcquireControlParams, AdoptNativeSessionParams, ControlLease, ControlLeaseParams,
    CoolSessionParams, EnrollmentDecision, EnrollmentReceipt, ForgetSessionParams,
    GetProviderCapabilitiesParams, GetSessionParams, InitializeParams, InitializeResult,
    IntegrationGrant, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, LaggedNotification,
    ListModelsParams, ListNativeSessionsParams, ListPendingApprovalsParams, ManagedSessionList,
    NativeSessionCatalogue, PendingApprovalList, ProviderList, ProviderUsageList,
    ProviderWatchEndedNotification, ProvidersChangedNotification, RequestEnrollmentParams,
    RespondApprovalParams, ResumeSessionParams, RotateIntegrationKeyParams,
    RuntimeEventNotification, RuntimeLocatorRecord, RuntimeMethod, RuntimeModelCatalog,
    RuntimeProviderCapabilities, ServerChallenge, SessionDescriptor,
    SessionIndexChangedNotification, SessionIndexEndedNotification, SessionOpenResult,
    StartSessionParams, SubmitInputParams, WatchEnrollmentParams, WatchEventsParams,
    WatchEventsResult, WatchProvidersParams, WatchProvidersResult, WatchSessionIndexParams,
    WatchSessionIndexResult,
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
    submit_input: SubmitInputParams,
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
    provider_watch_ended: ProviderWatchEndedNotification,
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
