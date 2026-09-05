//! View control uses fresh pinned authority and the existing lease mutation table, without a provider process.

use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::task::Poll;
use std::time::Duration;

use runtrol_runtime_protocol::{IntegrationId, ProviderId, WindowMirrorOpenParams};
use runtrol_store::IntegrationRootRow;

use super::*;

struct Fixture {
    composed: Arc<Composed>,
    view: TerminalView,
    row: IntegrationRow,
    feeder: crate::window_registry::ConnectionToken,
    directory: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let directory = std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("view-control-{}", TerminalId::now()));
        std::fs::create_dir(&directory).expect("create the unique fixture home");
        let workspace = directory.join("workspace");
        std::fs::create_dir(&workspace).expect("create the fixture workspace");
        let workspace = AbsPath::canonicalize(workspace.to_str().expect("UTF-8 workspace"))
            .expect("canonical workspace");
        let root = IntegrationRootRow {
            path: workspace.to_string().into(),
            identity: runtrol_security::ProjectRootIdentity::read(&workspace)
                .expect("read the approved root identity")
                .to_bytes(),
        };
        let row = IntegrationRow {
            public_key: [3; 32],
            client_instance_id: "view-control-fixture".into(),
            label: "View control fixture".into(),
            manifest_digest: [4; 32],
            scopes: vec!["session.input.write".into()],
            roots: vec![root.clone()],
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::now(),
            revoked_at: None,
        };
        let key = IntegrationKey::from_bytes([5; 16]);
        let composed = Arc::new(
            Composed::for_tests(
                directory.to_str().expect("UTF-8 fixture home"),
                runtrol_drivers::builtin(),
            )
            .expect("compose the in-memory terminal fixture"),
        );
        composed
            .integration_authority
            .publish_committed(key, row.clone())
            .expect("publish the fixture authority");
        let authority = AuthorizedIntegration {
            key,
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("fixture"),
                scopes: vec![AppScope::SessionInputWrite],
                roots: vec![workspace.to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
            roots: vec![root],
        };
        let feeder = crate::window_registry::ConnectionToken::next();
        let terminal = crate::terminal_surface::open_observed_mirror(
            &composed,
            feeder,
            "fixture-window".into(),
            WindowMirrorOpenParams {
                window_session_id: "fixture-window".into(),
                terminal_key: "fixture-terminal".into(),
                execution_id: "fixture-execution".into(),
                provider_id: ProviderId::new("claude"),
                command_line: "fixture".into(),
                cwd: workspace.to_string(),
                process_id: None,
                geometry: TerminalGeometry {
                    columns: 80,
                    rows: 24,
                },
            },
        )
        .await
        .expect("register a fed terminal without starting a process");
        let view = composed
            .runtime_terminals
            .attach(
                &composed,
                authority,
                &TerminalAttachParams {
                    terminal_id: terminal.to_string().parse().expect("public terminal ID"),
                },
            )
            .await
            .expect("attach an exact view");
        let mut fixture = Self {
            composed,
            view,
            row,
            feeder,
            directory,
        };
        // Composing the fixture may be slow under concurrent test load. Start the action with a real,
        // newly completed root proof, preserving its worker timestamp exactly as the relay does.
        fixture.refresh_proof().await;
        fixture
    }

    async fn refresh_proof(&mut self) {
        #[cfg(windows)]
        let check = {
            let guard = self.view.pinned_root_guard();
            move || guard.blocking_lock().valid()
        };
        #[cfg(not(windows))]
        let check = {
            let row = self.row.clone();
            let workspace = self.view.hosted.workspace.clone();
            move || validate_workspace_roots(&row, &workspace).is_ok()
        };
        let proof = run_root_check(self.composed.terminal_root_checks.clone(), check)
            .await
            .expect("the fixture root check completes in its bounded lane");
        assert!(proof.value, "the exact fixture root remains valid");
        self.view
            .remember_root_proof(proof.completed_at)
            .expect("the newly completed fixture proof is still fresh");
    }

    fn acquire_params(&self) -> TerminalAcquireControlParams {
        TerminalAcquireControlParams {
            request_id: MutationRequestId::now(),
            terminal_id: self.view.opened.terminal.terminal_id.clone(),
            expected_terminal_generation: self.view.hosted.generation,
        }
    }

    async fn acquire(
        &self,
        params: &TerminalAcquireControlParams,
    ) -> Result<TerminalControlLease, TerminalRuntimeFailure> {
        self.composed
            .runtime_terminals
            .acquire_view(&self.composed, &self.view, params)
            .await
    }

    async fn close(self) {
        crate::terminal_surface::end_observed_mirrors_of(&self.composed, self.feeder).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&self.composed) > 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exact fixture's exit observer finishes");
        drop(self.view);
        drop(self.composed);
        std::fs::remove_dir_all(self.directory).expect("remove the exact closed fixture directory");
    }
}

fn renewal(lease: &TerminalControlLease) -> TerminalControlParams {
    TerminalControlParams {
        request_id: MutationRequestId::now(),
        terminal_id: lease.terminal_id.clone(),
        lease_id: lease.lease_id.clone(),
        lease_generation: lease.lease_generation,
    }
}

#[tokio::test]
async fn applying_a_proof_keeps_its_actual_age_and_rejects_an_old_success() {
    let mut fixture = Fixture::new().await;
    let completed_at = Instant::now()
        .checked_sub(Duration::from_millis(500))
        .expect("the fixture clock supports a recent proof");
    fixture
        .view
        .remember_root_proof(completed_at)
        .expect("still fresh");
    assert_eq!(fixture.view.last_root_proof, completed_at);
    assert_eq!(
        fixture.view.remember_root_proof(
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .expect("the fixture clock supports a stale proof"),
        ),
        Err(RootCheckFailure::Stale)
    );
    assert_eq!(
        fixture.view.last_root_proof, completed_at,
        "refusal never changes the stored proof"
    );
    fixture.close().await;
}

#[cfg(windows)]
#[tokio::test]
async fn view_acquire_and_renew_use_the_recent_pinned_proof_without_reopening_the_path() {
    let fixture = Fixture::new().await;
    let params = fixture.acquire_params();
    let original = fixture.view.hosted.workspace.as_std_path();
    let renamed = fixture.directory.join("renamed");
    std::fs::rename(original, &renamed)
        .expect("move the exact fixture root after its recent proof");
    // The existing contract grants at most one second after a completed proof. A view uses that same proof;
    // the ordinary method must open the path again and therefore refuses this missing original path.
    let result = async {
        let lease = fixture.acquire(&params).await?;
        let renewed = fixture
            .composed
            .runtime_terminals
            .renew_view(&fixture.composed, &fixture.view, &renewal(&lease))
            .await?;
        let ordinary = fixture
            .composed
            .runtime_terminals
            .acquire(
                &fixture.composed,
                &fixture.view.authority,
                &fixture.acquire_params(),
            )
            .await;
        assert_eq!(
            ordinary
                .expect_err("ordinary admission still reads the root")
                .kind,
            RuntimeErrorKind::RootDenied
        );
        Ok::<_, TerminalRuntimeFailure>((lease, renewed))
    }
    .await;
    std::fs::rename(&renamed, original).expect("restore the exact fixture root");
    let (lease, renewed) = result.expect("fresh view control never reopens the root");
    assert!(renewed.lease_generation > lease.lease_generation);
    fixture.close().await;
}

#[tokio::test]
async fn view_control_requires_the_exact_terminal_and_process_generation() {
    let mut fixture = Fixture::new().await;
    let mut params = fixture.acquire_params();
    params.terminal_id = TerminalId::now()
        .to_string()
        .parse()
        .expect("another terminal ID");
    assert_eq!(
        fixture
            .acquire(&params)
            .await
            .expect_err("wrong view target")
            .kind,
        RuntimeErrorKind::TerminalNotFound
    );
    params = fixture.acquire_params();
    params.expected_terminal_generation += 1;
    assert_eq!(
        fixture
            .acquire(&params)
            .await
            .expect_err("wrong requested generation")
            .kind,
        RuntimeErrorKind::SessionConflict
    );
    params = fixture.acquire_params();
    fixture.view.hosted.generation += 1;
    assert_eq!(
        fixture
            .acquire(&params)
            .await
            .expect_err("old view generation")
            .kind,
        RuntimeErrorKind::TerminalGone
    );
    assert!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .is_empty()
    );
    fixture.close().await;
}

#[tokio::test]
async fn stale_view_authority_cannot_acquire_renew_or_replay_a_lease() {
    let mut fixture = Fixture::new().await;
    let params = fixture.acquire_params();
    let lease = fixture
        .acquire(&params)
        .await
        .expect("current view acquires");
    fixture.view.last_root_proof = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .expect("the fixture clock supports a stale proof");
    assert_eq!(
        fixture
            .acquire(&params)
            .await
            .expect_err("stale replay")
            .kind,
        RuntimeErrorKind::RootDenied
    );
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .renew_view(&fixture.composed, &fixture.view, &renewal(&lease),)
            .await
            .expect_err("stale renewal")
            .kind,
        RuntimeErrorKind::RootDenied
    );
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&fixture.view.hosted.id)
            .expect("the refusal preserves the existing lease")
            .lease_generation,
        lease.lease_generation
    );
    fixture.close().await;
}

#[tokio::test]
async fn a_queued_view_acquire_rechecks_authority_after_the_state_lock() {
    let fixture = Fixture::new().await;
    let params = fixture.acquire_params();
    let state = fixture.composed.runtime_terminals.state.lock().await;
    let mut acquiring = Box::pin(fixture.acquire(&params));
    poll_fn(|cx| {
        assert!(
            acquiring.as_mut().poll(cx).is_pending(),
            "control waits for the state lock"
        );
        Poll::Ready(())
    })
    .await;
    let mut revoked_scope = fixture.row.clone();
    revoked_scope.grant_generation += 1;
    revoked_scope.scopes.clear();
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.view.authority.key, revoked_scope)
        .expect("withdraw input scope while control is queued");
    drop(state);
    assert_eq!(
        acquiring
            .await
            .expect_err("queued control lost its scope")
            .kind,
        // A shrinking grant requires authenticated reconnect before another request is admitted.
        RuntimeErrorKind::Unauthenticated
    );
    assert!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .is_empty()
    );
    fixture.close().await;
}

#[tokio::test]
async fn a_queued_view_renewal_cannot_cross_a_changed_root_grant() {
    let fixture = Fixture::new().await;
    let lease = fixture
        .acquire(&fixture.acquire_params())
        .await
        .expect("current lease");
    let params = renewal(&lease);
    let state = fixture.composed.runtime_terminals.state.lock().await;
    let mut renewing = Box::pin(fixture.composed.runtime_terminals.renew_view(
        &fixture.composed,
        &fixture.view,
        &params,
    ));
    poll_fn(|cx| {
        assert!(
            renewing.as_mut().poll(cx).is_pending(),
            "renewal waits for the state lock"
        );
        Poll::Ready(())
    })
    .await;
    let mut changed = fixture.row.clone();
    changed.grant_generation += 1;
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.view.authority.key, changed)
        .expect("change the root grant while renewal is queued");
    drop(state);
    assert_eq!(
        renewing
            .await
            .expect_err("queued renewal has no proof for the new grant")
            .kind,
        RuntimeErrorKind::RootDenied
    );
    assert_eq!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .get(&fixture.view.hosted.id)
            .expect("the refusal preserves the existing lease")
            .lease_generation,
        lease.lease_generation
    );
    fixture.close().await;
}

#[tokio::test]
async fn a_proof_that_expires_while_control_is_queued_never_mints_a_lease() {
    let fixture = Fixture::new().await;
    let params = fixture.acquire_params();
    let state = fixture.composed.runtime_terminals.state.lock().await;
    let mut acquiring = Box::pin(fixture.acquire(&params));
    poll_fn(|cx| {
        assert!(
            acquiring.as_mut().poll(cx).is_pending(),
            "control waits for the state lock"
        );
        Poll::Ready(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    drop(state);
    assert_eq!(
        acquiring
            .await
            .expect_err("proof expired in the queue")
            .kind,
        RuntimeErrorKind::RootDenied
    );
    assert!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .is_empty()
    );
    fixture.close().await;
}
