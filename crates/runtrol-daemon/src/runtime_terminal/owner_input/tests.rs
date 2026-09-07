//! Exact owner input admission, delivery, refusal, and idempotency without any provider process.

#![cfg(test)]

use super::*;
use crate::runtime_terminal::view_control_tests::Fixture;
use crate::window_registry::{ConnectionToken, input::InputReceiver};
use runtrol_runtime_protocol::{
    MutationRequestId, ObservedCommand, ObservedTerminal, ProviderId, TerminalAcquireControlParams,
    TerminalAttachParams, TerminalControlLease, TerminalGeometry, WatchWindowInputParams,
    WindowMirrorOpenParams, WindowRegisterParams, WindowUpdateParams,
};
use std::sync::Arc;

struct InputFixture {
    base: Fixture,
    view: TerminalView,
    receiver: InputReceiver,
    lease: TerminalControlLease,
    registration: ConnectionToken,
    feeder: ConnectionToken,
}

fn observed(execution: &str) -> WindowUpdateParams {
    WindowUpdateParams {
        terminals: vec![ObservedTerminal {
            terminal_key: "input-terminal".into(),
            name: "Input fixture".into(),
            process_id: Some(std::process::id()),
            shell_integration: true,
            cwd: None,
            command: Some(ObservedCommand {
                execution_id: execution.into(),
                command_line: "fixture".into(),
                confidence: 2,
                started_at_ms: 1,
            }),
        }],
    }
}

impl InputFixture {
    async fn new() -> Self {
        let base = Fixture::new().await;
        let registration = ConnectionToken::next();
        let proof = base
            .composed
            .windows
            .register_authorized(
                registration,
                WindowRegisterParams {
                    window_session_id: "input-window".into(),
                    host_generation: "input-host".into(),
                    vscode_version: "fixture".into(),
                    workspace_folders: vec![base.view.hosted.workspace.to_string()],
                    host_pid: None,
                },
                base.view.authority.clone(),
            )
            .await
            .unwrap();
        base.composed
            .windows
            .update(registration, observed("execution-1"))
            .await
            .unwrap();
        let feeder = ConnectionToken::next();
        let terminal = crate::terminal_surface::open_observed_mirror(
            &base.composed,
            feeder,
            "input-window".into(),
            WindowMirrorOpenParams {
                registration_generation: proof.registration_generation,
                owner_token: proof.owner_token.clone(),
                window_session_id: "input-window".into(),
                terminal_key: "input-terminal".into(),
                execution_id: "execution-1".into(),
                provider_id: ProviderId::new(base.view.hosted.provider.as_str()),
                command_line: "fixture".into(),
                cwd: base.view.hosted.workspace.to_string(),
                process_id: Some(std::process::id()),
                geometry: TerminalGeometry {
                    columns: 80,
                    rows: 24,
                },
            },
        )
        .await
        .unwrap();
        let view = base
            .composed
            .runtime_terminals
            .attach(
                &base.composed,
                base.view.authority.clone(),
                &TerminalAttachParams {
                    terminal_id: terminal.to_string().parse().unwrap(),
                },
            )
            .await
            .unwrap();
        let lease = base
            .composed
            .runtime_terminals
            .acquire_view(
                &base.composed,
                &view,
                &TerminalAcquireControlParams {
                    request_id: MutationRequestId::now(),
                    terminal_id: view.opened.terminal.terminal_id.clone(),
                    expected_terminal_generation: view.hosted.generation,
                },
            )
            .await
            .unwrap();
        let changes = base.composed.terminals.lock().await.change_sender();
        let receiver = base
            .composed
            .windows
            .watch_input(
                base.view.authority.clone(),
                WatchWindowInputParams {
                    window_session_id: "input-window".into(),
                    registration_generation: proof.registration_generation,
                    owner_token: proof.owner_token,
                },
                "owner-fixture-subscription".into(),
                changes,
            )
            .await
            .unwrap();
        Self {
            base,
            view,
            receiver,
            lease,
            registration,
            feeder,
        }
    }

    fn params(&self) -> TerminalSendTextParams {
        TerminalSendTextParams {
            request_id: MutationRequestId::now(),
            terminal_id: self.lease.terminal_id.clone(),
            lease_id: self.lease.lease_id.clone(),
            lease_generation: self.lease.lease_generation,
            text: "caller text \u{d55c}\u{ae00}\r\u{3}".into(),
        }
    }

    async fn offer(
        &mut self,
        params: TerminalSendTextParams,
    ) -> (
        OwnerOffer,
        tokio::task::JoinHandle<Result<TerminalTextReceipt, TerminalRuntimeFailure>>,
    ) {
        let composed = Arc::clone(&self.base.composed);
        let authority = self.view.authority.clone();
        let task = tokio::spawn(async move {
            composed
                .runtime_terminals
                .send_text(&composed, &authority, params, None)
                .await
        });
        let offer = tokio::time::timeout(Duration::from_secs(2), self.receiver.requests.recv())
            .await
            .unwrap()
            .unwrap();
        (offer, task)
    }

    async fn close(self) {
        drop(self.receiver);
        self.base
            .composed
            .windows
            .forget_connection(self.registration)
            .await;
        crate::terminal_surface::end_observed_mirrors_of(&self.base.composed, self.feeder).await;
        drop(self.view);
        self.base.close().await;
    }
}

#[tokio::test]
async fn owner_text_is_claimed_once_and_replays_only_its_structural_receipt() {
    let mut fixture = InputFixture::new().await;
    let params = fixture.params();
    let (mut offer, task) = fixture.offer(params.clone()).await;
    let claim = offer
        .claim(&fixture.base.composed, &fixture.receiver.subscription_id)
        .await
        .unwrap();
    assert_eq!(claim.text, params.text);
    assert!(
        offer
            .claim(&fixture.base.composed, &fixture.receiver.subscription_id)
            .await
            .is_err()
    );
    let received = WindowInputReceiptParams {
        subscription_id: fixture.receiver.subscription_id.clone(),
        sequence: offer.sequence,
        outcome: WindowInputOutcome::OwnerExtensionAccepted,
        reason: None,
    };
    offer.finish(&fixture.base.composed, &received).await;
    let receipt = task.await.unwrap().unwrap();
    let repeated = fixture
        .base
        .composed
        .runtime_terminals
        .send_text(
            &fixture.base.composed,
            &fixture.view.authority,
            params.clone(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(receipt, repeated);
    assert!(
        !serde_json::to_string(&receipt)
            .unwrap()
            .contains(&params.text)
    );
    assert!(
        fixture.receiver.requests.try_recv().is_err(),
        "retry must not offer or invoke owner input again"
    );
    let mut conflicting = params;
    conflicting.text.push('!');
    assert_eq!(
        fixture
            .base
            .composed
            .runtime_terminals
            .send_text(
                &fixture.base.composed,
                &fixture.view.authority,
                conflicting,
                None
            )
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::IdempotencyConflict
    );
    fixture.close().await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryEnd {
    Lease,
    Execution,
    Birth,
    Grant,
    LostReceipt,
}

#[tokio::test]
async fn an_admitted_owner_refusal_or_lost_receipt_never_becomes_a_retryable_lease_error() {
    for changed in [
        DeliveryEnd::Lease,
        DeliveryEnd::Execution,
        DeliveryEnd::Birth,
        DeliveryEnd::Grant,
        DeliveryEnd::LostReceipt,
    ] {
        let mut fixture = InputFixture::new().await;
        let params = fixture.params();
        let (mut offer, task) = fixture.offer(params.clone()).await;
        match changed {
            DeliveryEnd::Lease => {
                fixture
                    .base
                    .composed
                    .runtime_terminals
                    .state
                    .lock()
                    .await
                    .leases
                    .get_mut(&fixture.view.hosted.id)
                    .unwrap()
                    .expires_at_ms = 0;
            }
            DeliveryEnd::Execution => fixture
                .base
                .composed
                .windows
                .update(fixture.registration, observed("execution-2"))
                .await
                .unwrap(),
            DeliveryEnd::Birth => {
                offer.shell = ProcessIdentity::new(
                    offer.shell.pid(),
                    offer.shell.started().saturating_add(1),
                )
                .unwrap();
            }
            DeliveryEnd::Grant => {
                let mut row = fixture
                    .base
                    .composed
                    .integration_authority
                    .row(fixture.view.authority.key)
                    .unwrap()
                    .as_ref()
                    .clone();
                row.grant_generation += 1;
                row.scopes.clear();
                fixture
                    .base
                    .composed
                    .integration_authority
                    .publish_committed(fixture.view.authority.key, row)
                    .unwrap();
            }
            DeliveryEnd::LostReceipt => {
                offer
                    .claim(&fixture.base.composed, &fixture.receiver.subscription_id)
                    .await
                    .unwrap();
            }
        }
        if changed != DeliveryEnd::LostReceipt {
            assert!(
                offer
                    .claim(&fixture.base.composed, &fixture.receiver.subscription_id)
                    .await
                    .is_err(),
                "{changed:?} must never disclose text"
            );
        }
        drop(offer);
        let outcome = task.await;
        assert_eq!(
            outcome.unwrap().unwrap_err().kind,
            RuntimeErrorKind::OutcomeUnknown
        );
        assert_eq!(
            fixture
                .base
                .composed
                .runtime_terminals
                .send_text(
                    &fixture.base.composed,
                    &fixture.view.authority,
                    params,
                    None
                )
                .await
                .unwrap_err()
                .kind,
            RuntimeErrorKind::OutcomeUnknown,
            "{changed:?} must retain admission without replay"
        );
        fixture.close().await;
    }
}
#[tokio::test]
async fn another_integration_cannot_replace_an_owner_and_receiver_drop_retracts_input() {
    let fixture = InputFixture::new().await;
    assert!(
        input_available(
            &fixture.base.composed,
            &fixture.view.authority,
            &fixture.view.hosted
        )
        .await
    );
    let mut stranger = fixture.view.authority.clone();
    stranger.key = runtrol_store::IntegrationKey::from_bytes([71; 16]);
    let replacing = WindowRegisterParams {
        window_session_id: "input-window".into(),
        host_generation: "replacement-host".into(),
        vscode_version: "fixture".into(),
        workspace_folders: vec![fixture.view.hosted.workspace.to_string()],
        host_pid: None,
    };
    let refused = fixture
        .base
        .composed
        .windows
        .register_authorized(ConnectionToken::next(), replacing, stranger)
        .await;
    assert!(matches!(refused, Err(failure) if failure.kind == RuntimeErrorKind::ScopeDenied));
    assert!(
        fixture
            .base
            .composed
            .windows
            .input_is_current(&fixture.receiver)
            .await
    );
    let mut changes = fixture.base.composed.terminals.lock().await.changes();
    changes.borrow_and_update();
    let InputFixture {
        base,
        view,
        receiver,
        registration,
        feeder,
        ..
    } = fixture;
    drop(receiver);
    assert!(
        changes.has_changed().unwrap(),
        "receiver retirement must push the existing terminal index"
    );
    assert!(!input_available(&base.composed, &view.authority, &view.hosted).await);
    base.composed.windows.forget_connection(registration).await;
    crate::terminal_surface::end_observed_mirrors_of(&base.composed, feeder).await;
    drop(view);
    base.close().await;
}

#[tokio::test]
async fn cancelling_the_caller_prevents_claim_and_preserves_unknown_without_replay() {
    let mut fixture = InputFixture::new().await;
    let params = fixture.params();
    let (mut offer, task) = fixture.offer(params.clone()).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        offer
            .claim(&fixture.base.composed, &fixture.receiver.subscription_id)
            .await
            .is_err()
    );
    drop(offer);
    assert_eq!(
        fixture
            .base
            .composed
            .runtime_terminals
            .send_text(
                &fixture.base.composed,
                &fixture.view.authority,
                params,
                None
            )
            .await
            .unwrap_err()
            .kind,
        RuntimeErrorKind::OutcomeUnknown
    );
    fixture.close().await;
}
