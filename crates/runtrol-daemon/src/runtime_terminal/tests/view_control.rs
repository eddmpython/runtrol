//! View control uses fresh pinned authority and the existing lease mutation table, without a provider process.

use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::task::Poll;
use std::time::{Duration, Instant};

use runtrol_runtime_protocol::{IntegrationId, ProviderId, WindowMirrorOpenParams};
use runtrol_store::IntegrationRootRow;

use super::*;

pub(crate) struct Fixture {
    pub(crate) composed: Arc<Composed>,
    pub(crate) view: TerminalView,
    row: IntegrationRow,
    feeder: crate::window_registry::ConnectionToken,
    directory: PathBuf,
}

impl Fixture {
    pub(crate) async fn new() -> Self {
        // A serving Runtime has measured its image before admission. Match that startup boundary so
        // the first fixture does not hash the entire test executable after pinning its one-second proof.
        runtime_generation().expect("the fixture Runtime has its startup generation");
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
                registration_generation: 0,
                owner_token: String::new(),
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
        let proof = Arc::clone(self.view.root_proof());
        tokio::task::spawn_blocking(move || proof.checked_now())
            .await
            .expect("the fixture root check worker finishes")
            .expect("the newly completed fixture proof is still fresh");
    }

    fn acquire_params(&self) -> TerminalAcquireControlParams {
        TerminalAcquireControlParams {
            request_id: MutationRequestId::now(),
            terminal_id: self.view.opened.terminal.terminal_id.clone(),
            expected_terminal_generation: self.view.hosted.generation,
        }
    }

    pub(crate) async fn another_terminal_view(&self, index: usize) -> TerminalView {
        let cwd = self
            .view
            .hosted
            .workspace
            .as_std_path()
            .join(format!("child-{index}"));
        std::fs::create_dir(&cwd).expect("create a distinct child workspace");
        let terminal = crate::terminal_surface::open_observed_mirror(
            &self.composed,
            self.feeder,
            "fixture-window".into(),
            WindowMirrorOpenParams {
                registration_generation: 0,
                owner_token: String::new(),
                window_session_id: "fixture-window".into(),
                terminal_key: format!("terminal-{index}"),
                execution_id: format!("execution-{index}"),
                provider_id: ProviderId::new("claude"),
                command_line: "fixture".into(),
                cwd: cwd.to_str().expect("UTF-8 child workspace").to_owned(),
                process_id: None,
                geometry: TerminalGeometry {
                    columns: 80,
                    rows: 24,
                },
            },
        )
        .await
        .expect("open a distinct fed terminal without a process");
        self.composed
            .runtime_terminals
            .attach(
                &self.composed,
                self.view.authority.clone(),
                &TerminalAttachParams {
                    terminal_id: terminal.to_string().parse().expect("terminal ID"),
                },
            )
            .await
            .expect("attach the distinct terminal")
    }

    pub(crate) async fn enable_output(&mut self) {
        self.row.scopes.push("session.output.read".into());
        self.row.grant_generation += 1;
        self.view
            .authority
            .grant
            .scopes
            .push(AppScope::SessionOutputRead);
        self.view.authority.grant.grant_generation = self.row.grant_generation;
        self.composed
            .integration_authority
            .publish_committed(self.view.authority.key, self.row.clone())
            .expect("publish output authority");
        self.view
            .refresh_root_authority()
            .await
            .expect("pin output authority");
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

    pub(crate) async fn close(self) {
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

fn input_params(lease: &TerminalControlLease) -> TerminalWriteParams {
    TerminalWriteParams {
        request_id: MutationRequestId::now(),
        terminal_id: lease.terminal_id.clone(),
        lease_id: lease.lease_id.clone(),
        lease_generation: lease.lease_generation,
        bytes_base64: Base64::encode_string(b"\x1b[1;2R"),
    }
}

#[tokio::test]
async fn an_observed_mirror_refuses_exact_byte_input_without_a_delivery_owner() {
    for dedicated in [false, true] {
        let fixture = Fixture::new().await;
        let lease = fixture
            .acquire(&fixture.acquire_params())
            .await
            .expect("the exact mirror has a current control lease");
        let params = input_params(&lease);
        let result = if dedicated {
            fixture
                .composed
                .runtime_terminals
                .write_view(&fixture.composed, &fixture.view, &params)
                .await
        } else {
            fixture
                .composed
                .runtime_terminals
                .write(&fixture.composed, &fixture.view.authority, &params)
                .await
        };
        let key = mutation_key(fixture.view.authority.key, &params.request_id)
            .expect("the rejected input has an exact mutation identity");
        let untouched = !fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .mutations
            .contains_key(&key);
        fixture.close().await;
        assert_eq!(
            result
                .expect_err("a mirror cannot acknowledge bytes that no owner received")
                .kind,
            RuntimeErrorKind::InvalidRequest
        );
        assert!(untouched, "refused bytes never enter the mutation ledger");
    }
}

#[tokio::test]
async fn queued_input_rechecks_revocation_and_root_proof_before_recording_any_write() {
    for (dedicated, revoke) in [(true, false), (true, true), (false, true)] {
        let fixture = Fixture::new().await;
        let lease = fixture
            .acquire(&fixture.acquire_params())
            .await
            .expect("input lease");
        let params = input_params(&lease);
        let ordered = fixture
            .view
            .hosted
            .terminal
            .operation()
            .await
            .expect("hold the PTY lane");
        let mut writing = Box::pin(async {
            if dedicated {
                fixture
                    .composed
                    .runtime_terminals
                    .write_view(&fixture.composed, &fixture.view, &params)
                    .await
            } else {
                fixture
                    .composed
                    .runtime_terminals
                    .write(&fixture.composed, &fixture.view.authority, &params)
                    .await
            }
        });
        poll_fn(|cx| {
            assert!(
                writing.as_mut().poll(cx).is_pending(),
                "input waits at the exact PTY lane"
            );
            Poll::Ready(())
        })
        .await;
        if revoke {
            let mut row = fixture.row.clone();
            row.grant_generation += 1;
            row.scopes.clear();
            fixture
                .composed
                .integration_authority
                .publish_committed(fixture.view.authority.key, row)
                .expect("withdraw input authority during the queue wait");
        } else {
            fixture.view.root_proof().set_test_completion(
                Instant::now()
                    .checked_sub(Duration::from_secs(2))
                    .expect("fixture clock has two elapsed seconds"),
            );
        }
        drop(ordered);
        let result = writing.await;
        let key = mutation_key(fixture.view.authority.key, &params.request_id)
            .expect("exact mutation key");
        let untouched = !fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .mutations
            .contains_key(&key);
        fixture.close().await;
        assert_eq!(
            result.expect_err("queued input lost authority").kind,
            if revoke {
                RuntimeErrorKind::Unauthenticated
            } else {
                RuntimeErrorKind::RootDenied
            }
        );
        assert!(
            untouched,
            "denied input never reserves a mutation or reaches the writer"
        );
    }
}

#[tokio::test]
async fn queued_input_checks_lease_expiry_after_the_final_state_lock() {
    let fixture = Fixture::new().await;
    let lease = fixture
        .acquire(&fixture.acquire_params())
        .await
        .expect("input lease");
    let params = input_params(&lease);
    let ordered = fixture
        .view
        .hosted
        .terminal
        .operation()
        .await
        .expect("hold the PTY lane");
    let mut writing = Box::pin(fixture.composed.runtime_terminals.write_view(
        &fixture.composed,
        &fixture.view,
        &params,
    ));
    poll_fn(|cx| {
        assert!(
            writing.as_mut().poll(cx).is_pending(),
            "first wait is at the PTY lane"
        );
        Poll::Ready(())
    })
    .await;
    let mut state = fixture.composed.runtime_terminals.state.lock().await;
    drop(ordered);
    poll_fn(|cx| {
        assert!(
            writing.as_mut().poll(cx).is_pending(),
            "the admitted operation now waits for final authority"
        );
        Poll::Ready(())
    })
    .await;
    state
        .leases
        .get_mut(&fixture.view.hosted.id)
        .expect("held lease")
        .expires_at_ms = WallMs::now().as_millis() + 30;
    tokio::time::sleep(Duration::from_millis(60)).await;
    drop(state);
    let result = writing.await;
    fixture.close().await;
    assert_eq!(
        result.expect_err("the lease expired inside the queue").kind,
        RuntimeErrorKind::LeaseExpired
    );
}

#[cfg(windows)]
#[tokio::test]
async fn eight_views_of_one_approved_root_share_one_validation_owner() {
    let fixture = Fixture::new().await;
    let first = fixture.view.pinned_root_guard();
    let mut views = Vec::new();
    let mut shared = true;
    for index in 1..8 {
        let view = fixture.another_terminal_view(index).await;
        assert_ne!(view.hosted.id, fixture.view.hosted.id);
        shared &= Arc::ptr_eq(&first, &view.pinned_root_guard());
        views.push(view);
    }
    let proof = fixture.view.root_proof();
    let calls_before = proof.test_calls();
    proof.set_test_completion(
        Instant::now()
            .checked_sub(ROOT_REFRESH_AFTER)
            .expect("fixture age"),
    );
    let mut changed = proof.subscribe();
    let refresh_started_at = Instant::now();
    proof.refresh().expect("request the first terminal refresh");
    for view in &views {
        view.root_proof()
            .refresh()
            .expect("join the same root check");
    }
    while proof
        .fresh()
        .is_ok_and(|completed| completed < refresh_started_at)
    {
        changed
            .changed()
            .await
            .expect("the shared OS check finishes");
    }
    let completed_at = proof.fresh().expect("current completed proof");
    assert_eq!(
        proof.test_calls(),
        calls_before + 1,
        "eight distinct terminals perform one OS check"
    );
    for view in &views {
        assert_eq!(view.root_proof().fresh(), Ok(completed_at));
    }
    drop(views);
    drop(first);
    fixture.close().await;
    assert!(
        shared,
        "one root must not schedule eight duplicate validations"
    );
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
    let fixture = Fixture::new().await;
    let params = fixture.acquire_params();
    let lease = fixture
        .acquire(&params)
        .await
        .expect("current view acquires");
    fixture.view.root_proof().set_test_completion(
        Instant::now()
            .checked_sub(Duration::from_secs(2))
            .expect("the fixture clock supports a stale proof"),
    );
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
async fn a_changed_grant_gets_a_new_os_completion_and_a_denial_reaches_other_views() {
    let mut fixture = Fixture::new().await;
    let other = fixture.another_terminal_view(1).await;
    let previous = fixture.view.root_proof().fresh().expect("initial proof");
    fixture.row.grant_generation += 1;
    fixture.view.authority.grant.grant_generation = fixture.row.grant_generation;
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.view.authority.key, fixture.row.clone())
        .expect("publish changed grant");
    fixture
        .view
        .refresh_root_authority()
        .await
        .expect("pin the new grant");
    assert!(fixture.view.root_proof().fresh().expect("new grant proof") > previous);
    assert!(!Arc::ptr_eq(fixture.view.root_proof(), other.root_proof()));
    drop(other);
    let other = fixture.another_terminal_view(2).await;
    let original = fixture.view.hosted.workspace.as_std_path().to_owned();
    let moved = fixture.directory.join("moved-root");
    std::fs::rename(&original, &moved).expect("move the approved directory");
    let proof = Arc::clone(fixture.view.root_proof());
    let denied = tokio::task::spawn_blocking(move || proof.checked_now())
        .await
        .expect("observe changed root");
    let input = fixture
        .composed
        .runtime_terminals
        .acquire_view(
            &fixture.composed,
            &other,
            &TerminalAcquireControlParams {
                request_id: MutationRequestId::now(),
                terminal_id: other.opened.terminal.terminal_id.clone(),
                expected_terminal_generation: other.hosted.generation,
            },
        )
        .await;
    std::fs::rename(&moved, &original).expect("restore the exact fixture root");
    drop(other);
    fixture.close().await;
    assert_eq!(denied, Err(RootCheckFailure::Denied));
    assert_eq!(
        input.expect_err("another view must see the denial").kind,
        RuntimeErrorKind::RootDenied
    );
}

#[tokio::test]
async fn queued_view_admission_cannot_return_a_snapshot_or_initial_lease_after_proof_expiry() {
    for initial_control in [false, true] {
        let fixture = Fixture::new().await;
        let attachment = fixture.view.hosted.terminal.attach().await;
        let held = fixture.composed.runtime_terminals.state.lock().await;
        let mut creating = Box::pin(fixture.composed.runtime_terminals.finish_view(
            &fixture.composed,
            fixture.view.authority.clone(),
            fixture.view.hosted.id,
            attachment,
            initial_control,
        ));
        poll_fn(|context| {
            assert!(
                creating.as_mut().poll(context).is_pending(),
                "admission waits after pinning its root"
            );
            Poll::Ready(())
        })
        .await;
        fixture.view.root_proof().set_test_completion(
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .expect("expired admission proof"),
        );
        drop(held);
        let result = creating.await;
        let refusal = result.err().map(|failure| failure.kind);
        let leases = fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .len();
        fixture.close().await;
        assert_eq!(refusal, Some(RuntimeErrorKind::RootDenied));
        assert_eq!(leases, 0, "expired admission cannot mint an initial lease");
    }
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
