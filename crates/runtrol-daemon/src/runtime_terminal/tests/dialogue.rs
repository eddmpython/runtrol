//! Dialogue controls retain the exact input authority across queues and idempotent retries.

use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::task::Poll;
use std::time::Duration;

use runtrol_core::terminal::Terminal;
use runtrol_store::{IntegrationRevocation, IntegrationRootRow};

use super::*;

struct Directory(PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove the exact dialogue fixture directory");
    }
}

struct Fixture {
    composed: Composed,
    hosted: HostedTerminal,
    authority: AuthorizedIntegration,
    row: IntegrationRow,
    params: TerminalSetDialogueParams,
    // Fields drop in declaration order: close the store before removing its private directory.
    directory: Directory,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.hosted
            .terminal
            .end_feed(None)
            .expect("close the in-memory fixture's feeder");
    }
}

impl Fixture {
    async fn new() -> Self {
        let terminal_id = TerminalId::now();
        let directory = std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("dialogue-control-{terminal_id}"));
        std::fs::create_dir(&directory).expect("create the unique fixture home");
        let workspace = directory.join("workspace");
        std::fs::create_dir(&workspace).expect("create the approved workspace");
        let workspace = AbsPath::canonicalize(workspace.to_str().expect("UTF-8 workspace"))
            .expect("canonical fixture workspace");
        let root = IntegrationRootRow {
            path: workspace.as_str().into(),
            identity: runtrol_security::ProjectRootIdentity::read(&workspace)
                .expect("capture approved root identity")
                .to_bytes(),
        };
        let row = IntegrationRow {
            public_key: [3; 32],
            client_instance_id: "dialogue-control-fixture".into(),
            label: "Dialogue control fixture".into(),
            manifest_digest: [4; 32],
            scopes: vec!["session.input.write".into()],
            roots: vec![root.clone()],
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::now(),
            revoked_at: None,
        };
        let key = IntegrationKey::from_bytes([5; 16]);
        let composed = Composed::for_tests(
            directory.to_str().expect("UTF-8 fixture home"),
            runtrol_drivers::builtin(),
        )
        .expect("compose the fixture");
        composed
            .integration_authority
            .publish_committed(key, row.clone())
            .expect("publish the fixture authority");
        let authority = AuthorizedIntegration {
            key,
            grant: IntegrationGrant {
                integration_id: runtrol_runtime_protocol::IntegrationId::new("fixture"),
                scopes: vec![AppScope::SessionInputWrite],
                roots: vec![workspace.to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
            roots: vec![root],
        };
        let hosted = HostedTerminal {
            spawned: None,
            id: terminal_id,
            provider: CoreProviderId::parse("claude").expect("a fixture provider"),
            terminal: Terminal::fed(0, runtrol_childproc::PtySize { cols: 80, rows: 24 })
                .expect("create the in-memory terminal without a process"),
            workspace,
            native: None,
            opened_at_ms: WallMs::now().as_millis(),
            generation: 1,
            stopping: false,
            origin: crate::terminal_surface::TerminalOrigin::Owned,
        };
        let minted = composed
            .courier_gate
            .mint(terminal_id)
            .expect("mint authority");
        composed
            .courier_gate
            .launch(minted, || Ok::<_, ()>(((), None)))
            .await
            .expect("register the managed lifetime without a process");
        let lease = new_lease(key, hosted.generation, 1).expect("allocate the input lease");
        let params = TerminalSetDialogueParams {
            request_id: MutationRequestId::now(),
            terminal_id: terminal_id
                .to_string()
                .parse()
                .expect("public terminal identity"),
            lease_id: lease.lease_id.clone(),
            lease_generation: lease.lease_generation,
            enabled: true,
        };
        composed
            .runtime_terminals
            .state
            .lock()
            .await
            .leases
            .insert(terminal_id, lease);
        Self {
            composed,
            hosted,
            authority,
            row,
            params,
            directory: Directory(directory),
        }
    }

    async fn apply(
        &self,
        params: &TerminalSetDialogueParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        self.composed
            .runtime_terminals
            .set_dialogue_hosted(&self.composed, &self.authority, params, &self.hosted)
            .await
    }

    async fn enabled(&self) -> bool {
        self.composed
            .courier_gate
            .dialogue_enabled(self.hosted.id)
            .await
    }

    fn publish(&self, row: IntegrationRow) {
        self.composed
            .integration_authority
            .publish_committed(self.authority.key, row)
            .expect("publish the exact changed grant");
    }
}

async fn assert_pending(
    future: std::pin::Pin<&mut impl Future<Output = Result<(), TerminalRuntimeFailure>>>,
) {
    let mut future = future;
    poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "the operation must be waiting"
        );
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn dialogue_retries_cannot_reapply_an_older_enable_or_disable() {
    let fixture = Fixture::new().await;
    assert!(
        !fixture.enabled().await,
        "a registered process starts disabled"
    );
    fixture
        .apply(&fixture.params)
        .await
        .expect("enable with current input authority");
    let mut disable = fixture.params.clone();
    disable.request_id = MutationRequestId::now();
    disable.enabled = false;
    fixture
        .apply(&disable)
        .await
        .expect("disable with current authority");
    fixture
        .apply(&fixture.params)
        .await
        .expect("replay the earlier successful enable");
    assert!(
        !fixture.enabled().await,
        "an old success must not reopen a lifetime"
    );

    let mut enable_again = fixture.params.clone();
    enable_again.request_id = MutationRequestId::now();
    fixture
        .apply(&enable_again)
        .await
        .expect("explicitly enable a new lifetime");
    fixture
        .apply(&disable)
        .await
        .expect("replay the old disable outcome");
    assert!(
        fixture.enabled().await,
        "an old disable cannot retire a newer lifetime"
    );
    let mut conflict = enable_again;
    conflict.enabled = false;
    assert_eq!(
        fixture
            .apply(&conflict)
            .await
            .expect_err("changed payload is not a retry")
            .kind,
        RuntimeErrorKind::IdempotencyConflict
    );
    assert!(fixture.enabled().await);
}

#[tokio::test]
async fn dialogue_rejects_replaced_expired_and_released_input_leases() {
    let fixture = Fixture::new().await;
    let mut state = fixture.composed.runtime_terminals.state.lock().await;
    let active = state
        .leases
        .get_mut(&fixture.hosted.id)
        .expect("current lease");
    active.lease_generation += 1;
    drop(state);
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("old generation")
            .kind,
        RuntimeErrorKind::ControlConflict
    );

    let mut state = fixture.composed.runtime_terminals.state.lock().await;
    let active = state
        .leases
        .get_mut(&fixture.hosted.id)
        .expect("current lease");
    active.lease_generation = fixture.params.lease_generation;
    active.owner = IntegrationKey::from_bytes([6; 16]);
    drop(state);
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("another holder")
            .kind,
        RuntimeErrorKind::ControlConflict
    );

    let mut state = fixture.composed.runtime_terminals.state.lock().await;
    let active = state
        .leases
        .get_mut(&fixture.hosted.id)
        .expect("current lease");
    active.owner = fixture.authority.key;
    active.expires_at_ms = WallMs::now().as_millis().saturating_sub(1);
    drop(state);
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("expired lease")
            .kind,
        RuntimeErrorKind::LeaseExpired
    );
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("retired lease")
            .kind,
        RuntimeErrorKind::LeaseExpired
    );
    assert!(!fixture.enabled().await);
    assert!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .mutations
            .is_empty(),
        "refusals cannot reserve an idempotency outcome"
    );
}

#[tokio::test]
async fn queued_dialogue_cannot_cross_scope_or_root_grant_revocation() {
    for revoke_scope in [true, false] {
        let fixture = Fixture::new().await;
        let operation = fixture
            .hosted
            .terminal
            .operation()
            .await
            .expect("hold the prior operation");
        let applying = fixture.apply(&fixture.params);
        tokio::pin!(applying);
        assert_pending(applying.as_mut()).await;
        let mut narrowed = fixture.row.clone();
        narrowed.grant_generation += 1;
        if revoke_scope {
            narrowed.scopes.clear();
        } else {
            narrowed.roots.clear();
        }
        fixture.publish(narrowed);
        drop(operation);
        assert_eq!(
            applying
                .await
                .expect_err("queued authority was revoked")
                .kind,
            RuntimeErrorKind::Unauthenticated
        );
        assert!(!fixture.enabled().await);
    }
}

#[tokio::test]
async fn queued_dialogue_cannot_cross_integration_revocation() {
    let fixture = Fixture::new().await;
    let operation = fixture
        .hosted
        .terminal
        .operation()
        .await
        .expect("hold the prior operation");
    let applying = fixture.apply(&fixture.params);
    tokio::pin!(applying);
    assert_pending(applying.as_mut()).await;
    fixture
        .composed
        .integration_authority
        .publish_revocation(
            fixture.authority.key,
            IntegrationRevocation {
                key_generation: 1,
                grant_generation: 2,
                revoked_at: WallMs::now(),
                order: 1,
            },
        )
        .expect("revoke the integration while its command waits");
    drop(operation);
    assert_eq!(
        applying
            .await
            .expect_err("queued integration was revoked")
            .kind,
        RuntimeErrorKind::IntegrationRevoked
    );
    assert!(!fixture.enabled().await);
}

#[tokio::test]
async fn dialogue_rejects_missing_input_scope_and_a_replaced_root() {
    let mut fixture = Fixture::new().await;
    fixture.authority.grant.scopes.clear();
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("no input scope")
            .kind,
        RuntimeErrorKind::ScopeDenied
    );
    fixture
        .authority
        .grant
        .scopes
        .push(AppScope::SessionInputWrite);
    std::fs::rename(
        fixture.directory.0.join("workspace"),
        fixture.directory.0.join("retired"),
    )
    .expect("retire the approved directory");
    std::fs::create_dir(fixture.directory.0.join("workspace"))
        .expect("replace the root at its old path");
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("replaced root")
            .kind,
        RuntimeErrorKind::RootDenied
    );
    assert!(!fixture.enabled().await);
}

#[tokio::test]
async fn observed_mirrors_cannot_enable_dialogue_even_with_a_current_lease() {
    let mut fixture = Fixture::new().await;
    fixture.hosted.origin = crate::terminal_surface::TerminalOrigin::ObservedMirror(Box::new(
        crate::terminal_surface::ObservedOwner {
            window_session_id: "fixture-window".to_owned(),
            terminal_key: "fixture-terminal".to_owned(),
            feeder: crate::window_registry::ConnectionToken::next(),
            shell_pid: None,
        },
    ));
    assert_eq!(
        fixture
            .apply(&fixture.params)
            .await
            .expect_err("a mirror has no managed courier")
            .kind,
        RuntimeErrorKind::InvalidRequest
    );
    assert!(!fixture.enabled().await);
}

async fn hold_gate(
    fixture: Arc<Fixture>,
) -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (started, starting) = tokio::sync::oneshot::channel();
    let (release, releasing) = std::sync::mpsc::channel();
    let runtime = tokio::runtime::Handle::current();
    let holding = tokio::task::spawn_blocking(move || {
        runtime.block_on(async {
            fixture
                .composed
                .courier_gate
                .set_dialogue_checked(fixture.hosted.id, false, None, || {
                    started.send(()).expect("announce the held gate lock");
                    releasing
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release the bounded fixture lock");
                    Ok::<_, ()>(())
                })
                .await
                .map_err(drop)
                .expect("release the unchanged gate state");
        });
    });
    starting.await.expect("the fixture holds the gate");
    (release, holding)
}

#[tokio::test]
async fn dialogue_checks_lease_expiry_after_the_final_gate_wait() {
    let fixture = Arc::new(Fixture::new().await);
    let (release, holding) = hold_gate(Arc::clone(&fixture)).await;
    let mut state = fixture.composed.runtime_terminals.state.lock().await;
    state
        .leases
        .get_mut(&fixture.hosted.id)
        .expect("input lease")
        .expires_at_ms = WallMs::now().as_millis().saturating_add(250);
    drop(state);
    let applying = fixture.apply(&fixture.params);
    tokio::pin!(applying);
    assert_pending(applying.as_mut()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    release
        .send(())
        .expect("finish the gate wait after lease expiry");
    holding.await.expect("join the bounded fixture worker");
    assert_eq!(
        applying
            .await
            .expect_err("the input lease expired during gate admission")
            .kind,
        RuntimeErrorKind::LeaseExpired
    );
    assert!(!fixture.enabled().await);
}

#[tokio::test]
async fn cancelling_a_dialogue_wait_cannot_mutate_or_poison_its_retry() {
    let fixture = Arc::new(Fixture::new().await);
    let (release, holding) = hold_gate(Arc::clone(&fixture)).await;
    {
        let applying = fixture.apply(&fixture.params);
        tokio::pin!(applying);
        assert_pending(applying.as_mut()).await;
    }
    release
        .send(())
        .expect("release after dropping the pending request");
    holding.await.expect("join the bounded fixture worker");
    assert!(
        !fixture.enabled().await,
        "cancellation precedes every state change"
    );
    assert!(
        fixture
            .composed
            .runtime_terminals
            .state
            .lock()
            .await
            .mutations
            .is_empty(),
        "a cancelled lock wait does not leave a pending outcome"
    );
    fixture
        .apply(&fixture.params)
        .await
        .expect("the exact cancelled request may retry");
    assert!(fixture.enabled().await);
}
