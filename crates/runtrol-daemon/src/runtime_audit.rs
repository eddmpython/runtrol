//! Content-free bounded audit composition for public Runtime and local integration administration.
//!
//! Every row goes to the durable store. A draining generation has handed that store to its successor, so its
//! rows go to the relay the successor takes on its next handoff poll (`audit_relay`). Refusing to serve
//! instead, which is what a released store used to mean here, retired the whole public surface of every
//! draining generation the moment the store moved (measured 2026-08-29).

use runtrol_provider::WallMs;
use runtrol_runtime_protocol::{AppScope, RuntimeMethod};
use runtrol_store::{IntegrationAuditOutcome, IntegrationAuditRow, IntegrationKey, StoreError};

use crate::compose::Composed;

/// Record one public Runtime authorization stage.
pub(crate) fn public(
    composed: &Composed,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: RuntimeMethod,
    scope: Option<AppScope>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
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
    composed: &Composed,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
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
    composed: &Composed,
    method: &'static str,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
        None,
        None,
        method,
        None,
        IntegrationAuditOutcome::Denied,
        reason,
    )
}

fn append(
    composed: &Composed,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    scope: Option<&'static str>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), StoreError> {
    let row = IntegrationAuditRow {
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
    };
    match composed.store.append_integration_audit(&row) {
        // Draining: the successor owns the store now and takes this row on its next poll.
        Err(StoreError::Released { .. }) => {
            composed.audit_relay.push(row);
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed_for(name: &str) -> (Composed, String) {
        let root = std::env::temp_dir().join(format!("runtrol-audit-{name}"));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        let text = root.to_str().expect("the scratch path is UTF-8").to_owned();
        // Composing without establishing containment: doing that in a test terminates the runner on one platform.
        let composed = Composed::for_tests(&text, runtrol_drivers::builtin())
            .expect("assemble a scratch home");
        (composed, text)
    }

    fn clean(composed: Composed, path: &str) {
        // The store owns an exclusive file handle. Release it before removing the scratch home.
        drop(composed);
        std::fs::remove_dir_all(path).expect("remove the scratch home");
    }

    #[test]
    fn a_row_goes_to_the_store_while_this_generation_holds_it() {
        let (composed, path) = composed_for("held");
        public(
            &composed,
            None,
            None,
            RuntimeMethod::TerminalsList,
            None,
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .expect("recorded");
        let rows = composed.store.list_integration_audit().expect("listed");
        assert!(
            rows.iter()
                .any(|row| &*row.method == RuntimeMethod::TerminalsList.as_str())
        );
        assert!(
            composed.audit_relay.take().is_empty(),
            "nothing waits for a successor"
        );
        clean(composed, &path);
    }

    #[test]
    fn a_draining_generation_keeps_the_row_for_its_successor_and_serves_on() {
        let (composed, path) = composed_for("drained");
        assert!(composed.store.release(), "the store is handed over once");

        public(
            &composed,
            None,
            None,
            RuntimeMethod::TerminalsAttach,
            None,
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .expect("a released store is not a refusal");

        let relayed = composed.audit_relay.take();
        assert_eq!(relayed.len(), 1);
        let kept = relayed.first().expect("the one row kept");
        assert_eq!(&*kept.method, RuntimeMethod::TerminalsAttach.as_str());
        clean(composed, &path);
    }
}
