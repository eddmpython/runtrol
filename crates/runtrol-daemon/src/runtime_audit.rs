//! Content-free bounded audit composition for public Runtime and local integration administration.

use runtrol_provider::WallMs;
use runtrol_runtime_protocol::{AppScope, RuntimeMethod};
use runtrol_store::{IntegrationAuditOutcome, IntegrationAuditRow, IntegrationKey, Store};

/// Record one public Runtime authorization stage.
pub(crate) fn public(
    store: &Store,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: RuntimeMethod,
    scope: Option<AppScope>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), runtrol_store::StoreError> {
    append(
        store,
        integration,
        key_generation,
        method.as_str(),
        scope.map(AppScope::as_str),
        outcome,
        reason,
    )
}

/// Record one private local integration administration stage.
pub(crate) fn local(
    store: &Store,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), runtrol_store::StoreError> {
    append(
        store,
        integration,
        key_generation,
        method,
        None,
        outcome,
        reason,
    )
}

/// Record a structurally invalid public envelope without retaining any caller-supplied method text.
pub(crate) fn structural(
    store: &Store,
    method: &'static str,
    reason: &'static str,
) -> Result<(), runtrol_store::StoreError> {
    append(
        store,
        None,
        None,
        method,
        None,
        IntegrationAuditOutcome::Denied,
        reason,
    )
}

fn append(
    store: &Store,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    scope: Option<&'static str>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), runtrol_store::StoreError> {
    store.append_integration_audit(&IntegrationAuditRow {
        occurred_at: WallMs::now(),
        integration,
        key_generation,
        method: method.into(),
        scope: scope.map(Into::into),
        project: None,
        session: None,
        request_id: None,
        outcome,
        reason: reason.into(),
    })
}
