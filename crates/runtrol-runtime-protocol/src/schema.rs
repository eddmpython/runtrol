//! Deterministic JSON schema derived from the Rust public DTO source of truth.

use schemars::{JsonSchema, schema_for};

use crate::{
    AcquireControlParams, ControlLease, ControlLeaseParams, EnrollmentDecision, EnrollmentReceipt,
    InitializeParams, InitializeResult, IntegrationGrant, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, LaggedNotification, ListModelsParams, ListNativeSessionsParams,
    ManagedSessionList, NativeSessionCatalogue, ProviderList, RequestEnrollmentParams,
    RuntimeEventNotification, RuntimeLocatorRecord, RuntimeMethod, RuntimeModelCatalog,
    ServerChallenge, SubmitInputParams, WatchEnrollmentParams, WatchEventsParams,
    WatchEventsResult,
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
    runtime_locator: RuntimeLocatorRecord,
    provider_list: ProviderList,
    list_models: ListModelsParams,
    model_catalogue: RuntimeModelCatalog,
    list_native_sessions: ListNativeSessionsParams,
    native_session_catalogue: NativeSessionCatalogue,
    managed_session_list: ManagedSessionList,
    acquire_control: AcquireControlParams,
    control_lease: ControlLease,
    control_lease_params: ControlLeaseParams,
    submit_input: SubmitInputParams,
    watch_events: WatchEventsParams,
    watch_events_result: WatchEventsResult,
    runtime_event: RuntimeEventNotification,
    lagged: LaggedNotification,
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
