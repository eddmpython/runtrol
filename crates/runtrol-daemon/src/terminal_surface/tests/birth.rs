//! Real suspended process birth must not serialize another terminal's input or courier command.

use super::*;
use std::time::Duration;

const READY: &str = "RUNTROL_PREPARED_BIRTH_FIXTURE";
const HELPER: &str = "terminal_surface::launch::tests::birth::prepared_birth_fixture";

#[test]
#[ignore = "private bounded process fixture launched only by birth ownership tests"]
fn prepared_birth_fixture() {
    let Some(marker) = std::env::var_os(READY) else {
        return;
    };
    std::fs::write(marker, std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(15));
}

fn native(fixture: &Fixture, id: TerminalId, resumed: bool) -> PreparedLaunch {
    let mut prepared = fixture.prepared(id, resumed);
    prepared.arguments = vec![
        "--exact".to_owned(),
        HELPER.to_owned(),
        "--ignored".to_owned(),
        "--nocapture".to_owned(),
    ];
    prepared.env.push((
        READY.to_owned(),
        fixture
            .scratch
            .root
            .join("birth-ready")
            .to_str()
            .unwrap()
            .to_owned(),
    ));
    prepared
}

type BirthReady = tokio::sync::oneshot::Receiver<runtrol_provider::ProcessIdentity>;
type BirthResult =
    tokio::task::JoinHandle<Result<(TerminalId, Terminal, Attachment), TerminalOpenError>>;

fn pause_birth(
    prepared: PreparedLaunch,
    composed: &Arc<Composed>,
    cancelled: Arc<AtomicBool>,
    fail_host: bool,
) -> (BirthReady, std::sync::mpsc::SyncSender<()>, BirthResult) {
    let (ready, receiving) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::sync_channel(1);
    let runtime = tokio::runtime::Handle::current();
    let operation = TerminalOperation::begin(composed);
    let composed = Arc::clone(composed);
    let running = tokio::task::spawn_blocking(move || {
        let _operation = operation;
        runtime.block_on(prepared.publish_prepared(&composed, &cancelled, |launch| {
            let mut birth = Birth::prepare(launch)?;
            let identity = runtrol_childproc::process_identity(birth.terminal.pid()).unwrap();
            eprintln!("owned prepared birth: identity={identity:?}");
            ready.send(identity).unwrap();
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            if fail_host {
                birth.failure = Some(TerminalOpenError::Provider(
                    "injected prepared-host failure".to_owned(),
                ));
            }
            Ok(birth)
        }))
    });
    (receiving, release, running)
}

fn public_authority(fixture: &Fixture, scope: AppScope) -> AuthorizedIntegration {
    let mut accepted = (*fixture.launch.authority).clone();
    let mut row = (*fixture
        .composed
        .integration_authority
        .row(accepted.key)
        .unwrap())
    .clone();
    if !accepted.grant.scopes.contains(&scope) {
        accepted.grant.scopes.push(scope);
        row.scopes.push("session.start".into());
    }
    row.grant_generation += 1;
    accepted.grant.grant_generation = row.grant_generation;
    fixture
        .composed
        .integration_authority
        .publish_committed(accepted.key, row)
        .unwrap();
    accepted
}

#[tokio::test]
async fn public_fresh_resume_and_official_birth_refuse_a_changed_grant_before_execution() {
    for (scope, official) in [
        (AppScope::SessionStart, false),
        (AppScope::SessionResume, false),
        (AppScope::SessionResume, true),
    ] {
        let fixture = Fixture::new().await;
        let accepted = public_authority(&fixture, scope);
        let id = TerminalId::now();
        let mut prepared = native(&fixture, id, false);
        prepared.authority = LaunchAuthority::integration(&accepted, scope);
        if official {
            prepared.minted = None;
            prepared.origin = super::super::super::TerminalOrigin::OfficialAttach(Box::new(
                super::super::super::OfficialStop {
                    containment: std::sync::Arc::new(runtrol_childproc::Containment::without_any()),
                    program: prepared.program.clone(),
                    arguments: Vec::new(),
                    cwd: prepared.cwd.clone(),
                    env: Vec::new(),
                    env_unset: Vec::new(),
                },
            ));
        }
        let (ready, release, running) = pause_birth(
            prepared,
            &fixture.composed,
            Arc::new(AtomicBool::new(false)),
            false,
        );
        let identity = ready.await.unwrap();
        // Even an otherwise equivalent new grant cannot activate an earlier accepted request.
        let mut row = (*fixture
            .composed
            .integration_authority
            .row(accepted.key)
            .unwrap())
        .clone();
        row.grant_generation += 1;
        fixture
            .composed
            .integration_authority
            .publish_committed(accepted.key, row)
            .unwrap();
        release.send(()).unwrap();
        let result = running.await.unwrap();
        fixture.close().await;
        assert!(result.is_err());
        assert!(!fixture.scratch.root.join("birth-ready").exists());
        assert!(!runtrol_childproc::matches_process_start(
            identity.pid(),
            identity.started()
        ));
        assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
    }
}

#[tokio::test]
async fn suspended_birth_allows_existing_input_and_courier_before_release() {
    use runtrol_courier::wire::{Answer, Request};
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
    let gate = &fixture.composed.courier_gate;
    gate.launch(gate.mint(other.id).unwrap(), || Ok::<_, ()>(((), None)))
        .await
        .unwrap();
    gate.set_dialogue(other.id, true).await.unwrap();
    let id = TerminalId::now();
    let (ready, release, running) = pause_birth(
        native(&fixture, id, false),
        &fixture.composed,
        Arc::new(AtomicBool::new(false)),
        false,
    );
    let identity = ready.await.unwrap();
    let input = tokio::time::timeout(
        Duration::from_secs(1),
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
    let courier = tokio::time::timeout(
        Duration::from_secs(1),
        gate.command(
            other.id.to_string().parse().unwrap(),
            Request::List { after: None },
        ),
    )
    .await;
    let unexecuted = !fixture.scratch.root.join("birth-ready").exists();
    release.send(()).unwrap();
    let opened = running.await.unwrap();
    let opened_ok = opened.is_ok();
    drop(opened);
    fixture.close().await;
    assert!(
        input.is_ok_and(|result| result.is_ok()),
        "input waited for unrelated birth"
    );
    assert!(
        matches!(courier, Ok(Answer::Sessions { .. })),
        "courier waited for unrelated birth"
    );
    assert!(unexecuted);
    assert!(opened_ok);
    assert!(!runtrol_childproc::matches_process_start(
        identity.pid(),
        identity.started()
    ));
}

#[tokio::test]
async fn pending_birth_counts_capacity_and_cancel_drain_revoke_failure_never_execute() {
    for interruption in ["cancel", "drain", "revoke", "failure"] {
        let fixture = Fixture::new().await;
        fixture.fill(MAX_HOSTED_TERMINALS - 1).await;
        let id = fixture.launch.owned.owner.terminal;
        let mut caller = Some(super::super::super::operations::LaunchCaller::new());
        let cancelled = Arc::clone(&caller.as_ref().unwrap().0);
        let (ready, release, running) = pause_birth(
            native(&fixture, id, true),
            &fixture.composed,
            Arc::clone(&cancelled),
            interruption == "failure",
        );
        let identity = ready.await.unwrap();
        let held = fixture.composed.terminals.lock().await.occupied();
        let operations = fixture.composed.terminal_operations.load(Ordering::Acquire);
        let ninth = fixture
            .prepared(TerminalId::now(), false)
            .publish(&fixture.composed, &AtomicBool::new(false), |_| {
                panic!("a ninth child must not be prepared")
            })
            .await;
        match interruption {
            "cancel" => drop(caller.take()),
            "drain" => fixture.composed.draining.store(true, Ordering::Release),
            "revoke" => fixture
                .composed
                .integration_authority
                .publish_revocation(
                    fixture.launch.authority.key,
                    runtrol_store::IntegrationRevocation {
                        key_generation: 1,
                        grant_generation: 2,
                        revoked_at: WallMs::now(),
                        order: 1,
                    },
                )
                .unwrap(),
            _ => {}
        }
        release.send(()).unwrap();
        let result = running.await.unwrap();
        // Retirement may already have completed before this task is scheduled again. A live root
        // must still hold its admission; an observed ended root may already have released it.
        let still_live =
            runtrol_childproc::matches_process_start(identity.pid(), identity.started());
        let retained = !fixture.composed.native_claims.terminal_absent(id).unwrap();
        fixture.close().await;
        assert_eq!(held, MAX_HOSTED_TERMINALS);
        assert_eq!(
            operations, 1,
            "the Runtime must retain its finite birth owner"
        );
        assert!(matches!(ninth, Err(TerminalOpenError::NoRoom { .. })));
        assert!(
            result.is_err(),
            "interrupted preparation executed: {interruption}"
        );
        assert!(
            !still_live || retained,
            "the live failed child lost its claim"
        );
        assert!(!fixture.scratch.root.join("birth-ready").exists());
        assert!(!runtrol_childproc::matches_process_start(
            identity.pid(),
            identity.started()
        ));
        assert!(fixture.composed.native_claims.terminal_absent(id).unwrap());
    }
}
