use super::*;
use crate::isolated_workspace::VerifiedProject;
use crate::isolated_workspace::ownership::{SpawnTicket, TerminalOwner};
use crate::isolated_workspace::tests::{Scratch, process::ProcessScratch};
use crate::native_claims::TerminalClaimAdmission;
use crate::runtime_auth::AuthorizedIntegration;
use crate::terminal_surface::ResumedTerminal;
use runtrol_provider::WallMs;
use runtrol_runtime_protocol::{AppScope, IntegrationGrant, IntegrationId};
use runtrol_store::{IntegrationKey, IntegrationRootRow, IntegrationRow};

struct Fixture {
    composed: Arc<Composed>,
    launch: ResumeLaunch,
    home: std::path::PathBuf,
    original_ticket: SpawnTicket,
    scratch: Scratch,
}

impl Fixture {
    async fn new() -> Self {
        let scratch = Scratch::make();
        let home = scratch.root.join("runtime");
        std::fs::create_dir(&home).unwrap();
        let composed = Arc::new(
            Composed::for_tests(home.to_str().unwrap(), runtrol_drivers::builtin()).unwrap(),
        );
        let runtime = runtrol_childproc::process_identity(std::process::id()).unwrap();
        let project = VerifiedProject::discover(&scratch.project).unwrap();
        let ticket = SpawnTicket::new(runtime, TerminalId::now(), TerminalId::now(), 1).unwrap();
        let mut original = ProcessScratch::start(&scratch);
        let binding = {
            let mut controller = composed.isolated_workspaces.lock().await;
            let prepared = controller
                .prepare_terminal(&composed.containment, &ticket, &project)
                .await
                .unwrap();
            controller
                .bind_terminal(&ticket, original.identity, &prepared.workspace)
                .unwrap();
            controller
                .resume_binding(&prepared.workspace)
                .unwrap()
                .unwrap()
        };
        original.stop();
        let root = IntegrationRootRow {
            path: project.root().to_string().into(),
            identity: project.root_identity(),
        };
        let key = IntegrationKey::from_bytes([17; 16]);
        composed
            .integration_authority
            .publish_committed(
                key,
                IntegrationRow {
                    public_key: [18; 32],
                    client_instance_id: "resume-birth-fixture".into(),
                    label: "Resume fixture".into(),
                    manifest_digest: [19; 32],
                    scopes: vec!["session.resume".into(), "session.input.write".into()],
                    roots: vec![root.clone()],
                    key_generation: 1,
                    grant_generation: 1,
                    approved_at: WallMs::now(),
                    revoked_at: None,
                },
            )
            .unwrap();
        let authority = Arc::new(AuthorizedIntegration {
            key,
            roots: vec![root],
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("resume-birth"),
                scopes: vec![AppScope::SessionResume, AppScope::SessionInputWrite],
                roots: vec![project.root().to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
        });
        let launch = ResumeLaunch {
            authority,
            owned: Arc::new(ResumedTerminal {
                binding,
                owner: TerminalOwner {
                    runtime: runtime.into(),
                    terminal: TerminalId::now(),
                },
            }),
        };
        Self {
            composed,
            launch,
            home,
            original_ticket: ticket,
            scratch,
        }
    }

    fn prepared(&self, terminal: TerminalId, resumed: bool) -> PreparedLaunch {
        let provider = ProviderId::parse("fixture").unwrap();
        let native: Box<str> = terminal.to_string().into();
        let cwd = if resumed {
            self.launch.owned.binding.workspace.clone()
        } else {
            self.scratch.project.clone()
        };
        let TerminalClaimAdmission::Reserved(reservation) = self
            .composed
            .native_claims
            .reserve_terminal(
                terminal,
                provider.as_str(),
                Some(&native),
                cwd.as_str(),
                false,
            )
            .unwrap()
        else {
            panic!("fixture claim must reserve");
        };
        PreparedLaunch {
            terminal_id: terminal,
            provider,
            native: Some(native),
            cwd,
            program: runtrol_childproc::resolve(std::env::current_exe().unwrap().to_str().unwrap())
                .unwrap(),
            arguments: Vec::new(),
            env: Vec::new(),
            env_unset: Vec::new(),
            size: PtySize { cols: 80, rows: 24 },
            minted: self.composed.courier_gate.mint(terminal).unwrap(),
            reservation,
            worker: None,
            resumed: resumed.then(|| self.launch.clone()),
        }
    }

    async fn fill(&self, count: usize) {
        for _ in 0..count {
            let terminal = Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap();
            let id = TerminalId::now();
            self.composed.terminals.lock().await.insert(
                id,
                ProviderId::parse("fixture").unwrap(),
                None,
                terminal.clone(),
                self.scratch.project.clone(),
                None,
            );
            super::super::forget_on_exit(Arc::clone(&self.composed), id, &terminal);
        }
    }

    async fn close(&self) {
        for row in self.composed.terminals.lock().await.hosted_all() {
            row.terminal.kill().unwrap();
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let rows = self.composed.terminals.lock().await.hosted_all();
            let operations = self.composed.terminal_operations.load(Ordering::Acquire);
            if rows.is_empty() && operations == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let claim_absent = self
                    .composed
                    .native_claims
                    .terminal_absent(self.launch.owned.owner.terminal);
                let controller_free = self.composed.isolated_workspaces.try_lock().is_ok();
                let lock_free = |path| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(path)
                        .map(|file| file.try_lock().is_ok())
                };
                let registry_free = lock_free(self.home.join("isolated-workspaces.lock"));
                let uuid = uuid::Uuid::parse_str(&self.launch.owned.binding.workspace_id).unwrap();
                let stripe =
                    usize::from(*uuid.as_bytes().last().unwrap()) % runtrol_core::session::MAX_HOT;
                let stripe_free = lock_free(
                    self.home
                        .join(format!("isolated-workspaces.operation-{stripe}.lock")),
                );
                let exited = rows
                    .iter()
                    .filter(|row| row.terminal.exited().borrow().is_some())
                    .count();
                panic!(
                    "fixture ownership cleanup deadline: rows={} exited={exited} operations={operations} claim_absent={claim_absent:?} controller_free={controller_free} registry_free={registry_free:?} stripe_free={stripe_free:?}",
                    rows.len()
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test]
async fn both_ended_processes_keep_the_worktree_until_their_exact_claims_retire() {
    let fixture = Fixture::new().await;
    let claim = |owner: TerminalOwner| {
        let TerminalClaimAdmission::Reserved(claim) = fixture
            .composed
            .native_claims
            .reserve_terminal(
                owner.terminal,
                "fixture",
                Some(&owner.terminal.to_string()),
                fixture.launch.owned.binding.workspace.as_str(),
                false,
            )
            .unwrap()
        else {
            panic!("fixture claim");
        };
        claim
    };
    let original = claim(fixture.original_ticket.worker);
    original.commit_born().unwrap();
    assert!(fixture.launch.reserve(&fixture.composed).await.is_err());
    fixture
        .composed
        .native_claims
        .terminal_ended(fixture.original_ticket.worker.terminal);
    let resumed_claim = claim(fixture.launch.owned.owner);
    resumed_claim.commit_born().unwrap();
    let mut reservation = fixture.launch.reserve(&fixture.composed).await.unwrap();
    let mut process = ProcessScratch::start(&fixture.scratch);
    reservation.bind(Some(process.identity)).unwrap();
    drop(reservation);
    process.stop();
    let old_exit = crate::isolated_workspace::ownership::EndedSpawn::after_gate_retired(
        fixture.original_ticket,
    );
    assert!(
        fixture
            .composed
            .isolated_workspaces
            .lock()
            .await
            .release_terminal_if_present(&fixture.composed.containment, &old_exit)
            .await
            .is_err(),
        "an original exit cannot reclaim underneath the resumed terminal cleanup claim"
    );
    let mut next = fixture.launch.clone();
    let mut owner = (*next.owned).clone();
    owner.owner.terminal = TerminalId::now();
    next.owned = Arc::new(owner);
    assert!(
        next.reserve(&fixture.composed).await.is_err(),
        "exited PID is not retired terminal ownership"
    );
    fixture
        .composed
        .native_claims
        .terminal_ended(fixture.launch.owned.owner.terminal);
    next.reserve(&fixture.composed)
        .await
        .unwrap()
        .abort()
        .unwrap();
}

#[tokio::test]
async fn owner_local_broker_preserves_original_launch_and_refuses_unbound_owned_cwds() {
    let fixture = Fixture::new().await;
    let provider = ProviderId::parse("codex").unwrap();
    let program =
        runtrol_childproc::resolve(std::env::current_exe().unwrap().to_str().unwrap()).unwrap();
    let child = fixture
        .launch
        .owned
        .binding
        .workspace
        .join("nested")
        .unwrap();
    std::fs::create_dir(child.as_std_path()).unwrap();
    for cwd in [fixture.launch.owned.binding.workspace.clone(), child] {
        let result = crate::terminal_surface::open_brokered(
            &fixture.composed,
            provider,
            cwd,
            80,
            24,
            vec!["--list".to_owned()],
            program.clone(),
        )
        .await;
        assert!(
            matches!(result, Err(TerminalOpenError::Provider(ref reason)) if reason.contains("Core-owned worktree"))
        );
        assert_eq!(fixture.composed.terminals.lock().await.len(), 0);
    }
    let opened = crate::terminal_surface::open_brokered(
        &fixture.composed,
        provider,
        fixture.scratch.project.clone(),
        80,
        24,
        vec!["--list".to_owned()],
        program,
    )
    .await
    .unwrap();
    drop(opened.2);
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_and_capacity_refusal_never_call_birth_and_clear_pending_occupancy() {
    let fixture = Fixture::new().await;
    let id = fixture.launch.owned.owner.terminal;
    assert!(
        fixture
            .prepared(id, true)
            .publish(&fixture.composed, &AtomicBool::new(true), |_| panic!(
                "cancelled birth"
            ))
            .await
            .is_err()
    );
    assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
    fixture
        .launch
        .reserve(&fixture.composed)
        .await
        .unwrap()
        .abort()
        .unwrap();
    fixture.fill(MAX_HOSTED_TERMINALS).await;
    assert!(matches!(
        fixture
            .prepared(id, true)
            .publish(&fixture.composed, &AtomicBool::new(false), |_| panic!(
                "over-capacity birth"
            ))
            .await,
        Err(TerminalOpenError::NoRoom { .. })
    ));
    fixture
        .launch
        .reserve(&fixture.composed)
        .await
        .unwrap()
        .abort()
        .unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn failed_host_publication_retains_native_claim_and_the_eighth_slot_until_exact_exit() {
    let fixture = Fixture::new().await;
    fixture.fill(MAX_HOSTED_TERMINALS - 1).await;
    let id = TerminalId::now();
    let prepared = fixture.prepared(id, false);
    let native = prepared.native.clone().unwrap();
    let result = prepared
        .publish(&fixture.composed, &AtomicBool::new(false), |_| {
            Err(TerminalError::CleanupIncomplete {
                cause: Box::new(TerminalError::Runtime(
                    "fixture reader setup failure".to_owned(),
                )),
                cleanup: runtrol_childproc::SpawnError::Pty {
                    doing: "fixture cleanup",
                    detail: "exact child still owned".to_owned(),
                },
                terminal: Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap(),
            })
        })
        .await;
    assert!(result.is_err());
    assert!(!fixture.composed.native_claims.terminal_absent(id).unwrap());
    assert_eq!(
        fixture.composed.terminals.lock().await.len(),
        MAX_HOSTED_TERMINALS
    );
    assert!(
        fixture
            .composed
            .native_claims
            .reserve_structured(
                runtrol_provider::SessionId::now(),
                "fixture",
                Some(&native),
                fixture.scratch.project.as_str()
            )
            .is_err()
    );
    assert!(matches!(
        fixture
            .prepared(TerminalId::now(), false)
            .publish(&fixture.composed, &AtomicBool::new(false), |_| panic!(
                "ninth birth"
            ))
            .await,
        Err(TerminalOpenError::NoRoom { .. })
    ));
    fixture.close().await;
    assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
}

#[tokio::test]
async fn failed_registry_bind_and_immediate_exit_use_the_same_exact_observer() {
    for immediate in [false, true] {
        let fixture = Fixture::new().await;
        let id = fixture.launch.owned.owner.terminal;
        let held = std::cell::RefCell::new(None);
        let result = fixture
            .prepared(id, true)
            .publish(&fixture.composed, &AtomicBool::new(false), |_| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(fixture.home.join("isolated-workspaces.lock"))
                    .unwrap();
                file.lock().unwrap();
                *held.borrow_mut() = Some(file);
                let terminal = Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap();
                if immediate {
                    terminal.end_feed(None).unwrap();
                }
                Ok(terminal)
            })
            .await;
        assert!(result.is_err());
        assert!(!fixture.composed.native_claims.terminal_absent(id).unwrap());
        assert!(fixture.composed.terminals.lock().await.hosted(id).is_some());
        drop(held.into_inner());
        eprintln!("fixture bind refusal cleanup: immediate={immediate}");
        fixture.close().await;
        assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
        assert!(
            !fixture
                .launch
                .owned
                .binding
                .workspace
                .as_std_path()
                .exists()
        );
    }
}

#[tokio::test]
async fn a_second_viewer_joins_the_existing_native_owner_before_reserving_the_worktree() {
    let fixture = Fixture::new().await;
    let existing = TerminalId::now();
    let provider = ProviderId::parse("fixture").unwrap();
    let terminal = Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap();
    fixture.composed.terminals.lock().await.insert(
        existing,
        provider,
        Some((provider, "native".into())),
        terminal.clone(),
        fixture.launch.owned.binding.workspace.clone(),
        Some("native".into()),
    );
    super::super::forget_on_exit(Arc::clone(&fixture.composed), existing, &terminal);
    let reservation = fixture.launch.reserve(&fixture.composed).await.unwrap();
    let joined = crate::terminal_surface::open_resumed(
        &fixture.composed,
        provider,
        "native",
        &fixture.launch,
        80,
        24,
        runtrol_childproc::resolve(std::env::current_exe().unwrap().to_str().unwrap()).unwrap(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(joined.0, existing);
    assert_eq!(fixture.composed.terminals.lock().await.len(), 1);
    reservation.abort().unwrap();
    drop(joined);
    fixture.close().await;
}

#[tokio::test]
async fn a_failed_rollback_requires_positive_retirement_of_the_exact_native_claim() {
    let fixture = Fixture::new().await;
    let old = fixture.launch.owned.owner.terminal;
    let prepared = fixture.prepared(old, true);
    let reservation = fixture.launch.reserve(&fixture.composed).await.unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(fixture.home.join("isolated-workspaces.lock"))
        .unwrap();
    file.lock().unwrap();
    assert!(reservation.abort().is_err());
    drop(file);
    let mut next = fixture.launch.clone();
    let mut owned = (*next.owned).clone();
    owned.owner.terminal = TerminalId::now();
    next.owned = Arc::new(owned);
    assert!(
        next.reserve(&fixture.composed).await.is_err(),
        "a pending claim is still an owner"
    );
    prepared.reservation.commit_born().unwrap();
    assert!(
        next.reserve(&fixture.composed).await.is_err(),
        "a born claim is still an owner"
    );
    fixture.composed.native_claims.terminal_ended(old);
    next.reserve(&fixture.composed)
        .await
        .unwrap()
        .abort()
        .unwrap();
}

#[tokio::test]
async fn another_terminal_accepts_input_before_failed_launch_cleanup_is_released() {
    use runtrol_runtime_protocol::{
        MutationRequestId, TerminalAcquireControlParams, TerminalWriteParams,
    };
    let fixture = Fixture::new().await;
    fixture.fill(1).await;
    let other = fixture
        .composed
        .terminals
        .lock()
        .await
        .hosted_all()
        .remove(0);
    let lease = fixture
        .composed
        .runtime_terminals
        .acquire(
            &fixture.composed,
            &fixture.launch.authority,
            &TerminalAcquireControlParams {
                request_id: MutationRequestId::now(),
                terminal_id: other.id.to_string().parse().unwrap(),
                expected_terminal_generation: other.generation,
            },
        )
        .await
        .unwrap();
    let cleanup_barrier = std::cell::RefCell::new(None);
    let id = fixture.launch.owned.owner.terminal;
    assert!(
        fixture
            .prepared(id, true)
            .publish(&fixture.composed, &AtomicBool::new(false), |_| {
                *cleanup_barrier.borrow_mut() =
                    Some(fixture.composed.isolated_workspaces.try_lock().unwrap());
                Ok(Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap())
            })
            .await
            .is_err()
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !fixture.composed.native_claims.terminal_absent(id).unwrap() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture exit observer deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(fixture.composed.terminal_operations.load(Ordering::Acquire) > 0);
    let written = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.composed.runtime_terminals.write(
            &fixture.composed,
            &fixture.launch.authority,
            &TerminalWriteParams {
                request_id: MutationRequestId::now(),
                terminal_id: other.id.to_string().parse().unwrap(),
                lease_id: lease.lease_id,
                lease_generation: lease.lease_generation,
                bytes_base64: "eA==".to_owned(),
            },
        ),
    )
    .await;
    assert!(
        written.is_ok_and(|result| result.is_ok()),
        "another terminal must accept input while cleanup is blocked"
    );
    assert!(cleanup_barrier.borrow().is_some());
    assert!(
        fixture
            .launch
            .owned
            .binding
            .workspace
            .as_std_path()
            .exists()
    );
    drop(cleanup_barrier.into_inner());
    fixture.close().await;
}

#[tokio::test]
async fn an_exit_completed_before_publication_is_seen_by_the_late_owner_observer() {
    let fixture = Fixture::new().await;
    let id = fixture.launch.owned.owner.terminal;
    let prepared = fixture.prepared(id, true);
    let composed = Arc::clone(&fixture.composed);
    let runtime = tokio::runtime::Handle::current();
    let result = tokio::task::spawn_blocking(move || {
        runtime.block_on(prepared.publish(&composed, &AtomicBool::new(false), |_| {
            let terminal = Terminal::fed(0, PtySize { cols: 80, rows: 24 }).unwrap();
            terminal.end_feed(Some(0)).unwrap();
            // No receiver exists while the real Core watcher observes and settles this exit.
            std::thread::sleep(std::time::Duration::from_millis(600));
            assert!(
                terminal.exited().borrow().is_some(),
                "exit must survive before daemon publication"
            );
            Ok(terminal)
        }))
    })
    .await
    .unwrap();
    assert!(
        result.is_err(),
        "the unidentifiable fixture root fails binding"
    );
    fixture.close().await;
    assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
    assert!(
        !fixture
            .launch
            .owned
            .binding
            .workspace
            .as_std_path()
            .exists()
    );
}
