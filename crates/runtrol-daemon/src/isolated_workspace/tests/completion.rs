//! The OS producer is exercised by childproc's keeper journey. These tests inject its private
//! structural result to verify the existing Git registry's independent CAS consumer.

use super::process::{ProcessScratch, current_process};
use super::{Scratch, git, git_output};
use crate::isolated_workspace::completion::{GenerationProof, proven};
use crate::isolated_workspace::ownership::{ProcessStamp, SpawnTicket, TerminalOwner};
use crate::isolated_workspace::{
    FILE_SCHEMA, File, IsolatedWorkspaceController, Record, State, VerifiedProject, registry,
};
use runtrol_childproc::Containment;
use runtrol_provider::TerminalId;

fn row(scratch: &Scratch) -> Record {
    registry::read(&scratch.registry)
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

async fn ended(scratch: &Scratch) -> (IsolatedWorkspaceController, SpawnTicket, ProcessStamp) {
    let controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut runtime = ProcessScratch::start(scratch);
    let mut worker = ProcessScratch::start(scratch);
    let ticket =
        SpawnTicket::new(runtime.identity, TerminalId::now(), TerminalId::now(), 1).unwrap();
    let workspace = controller
        .prepare_terminal(
            &Containment::without_any(),
            &ticket,
            &VerifiedProject::discover(&scratch.project).unwrap(),
        )
        .await
        .unwrap();
    controller
        .bind_terminal(&ticket, worker.identity, &workspace.workspace)
        .unwrap();
    let keeper = current_process().into();
    let mut record = row(scratch);
    record.lifetime = Some(GenerationProof {
        runtime: ticket.worker.runtime,
        keeper: Some(keeper),
        completed: false,
    });
    registry::update(&scratch.registry, record).unwrap();
    worker.stop();
    runtime.stop();
    (controller, ticket, keeper)
}

#[tokio::test]
async fn keeper_completion_requires_exact_owner_and_home_before_recovery_and_rejects_stale_cas() {
    let scratch = Scratch::make();
    let (controller, ticket, keeper) = ended(&scratch).await;
    let stale = row(&scratch);
    let containment = Containment::without_any();
    assert!(
        controller
            .recover_terminal(&containment, &ticket)
            .await
            .is_err()
    );
    let current = current_process();
    let wrong_keeper = runtrol_provider::ProcessIdentity::new(current.pid(), current.started() + 1)
        .unwrap()
        .into();
    registry::complete(
        &scratch.registry,
        ticket.worker.runtime,
        wrong_keeper,
        || Ok(()),
    )
    .unwrap();
    assert_eq!(row(&scratch).revision, stale.revision);
    assert!(
        registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Err(
            "home replaced".to_owned()
        ))
        .is_err()
    );
    assert!(!proven(row(&scratch).lifetime, ticket.worker.runtime));
    let mut checks = 0;
    assert!(
        registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || {
            checks += 1;
            if checks == 2 {
                Err("home changed before publication".to_owned())
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(row(&scratch).revision, stale.revision);
    registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Ok(())).unwrap();
    let completed = row(&scratch);
    assert!(proven(completed.lifetime, ticket.worker.runtime));
    assert_eq!(completed.revision, stale.revision + 1);
    assert!(registry::update(&scratch.registry, stale).is_err());
    registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Ok(())).unwrap();
    assert_eq!(
        row(&scratch).revision,
        completed.revision,
        "duplicate completion is idempotent"
    );
    controller
        .recover_terminal(&containment, &ticket)
        .await
        .unwrap();
    assert!(!completed.workspace.as_std_path().exists());
}

#[tokio::test]
async fn structured_release_requires_recorded_close_and_reopen_invalidates_it() {
    let scratch = Scratch::make();
    let controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let containment = Containment::without_any();
    let request = "01234567-89ab-cdef-0123-456789abcde0";
    let runtrol_ipc::Response::IsolatedWorkspace(prepared) = controller
        .prepare(&containment, request, scratch.project.as_str())
        .await
        .unwrap()
    else {
        panic!("prepared fixture workspace");
    };
    let workspace = runtrol_provider::AbsPath::canonicalize(&prepared.workspace).unwrap();
    let session = runtrol_provider::SessionId::now();
    controller.reopen_session(session, &workspace).unwrap();
    controller
        .bind(request, &session.to_string(), workspace.as_str())
        .unwrap();
    assert!(
        controller
            .release(&containment, Some(request), None, workspace.as_str())
            .await
            .is_err()
    );
    let unrelated =
        super::super::EndedSession::after_close_completed(runtrol_provider::SessionId::now())
            .unwrap();
    controller.complete_session(&unrelated).unwrap();
    assert!(!row(&scratch).session_completed);
    let ended = super::super::EndedSession::after_close_completed(session).unwrap();
    controller.complete_session(&ended).unwrap();
    let completed = row(&scratch);
    assert!(completed.session_completed);
    assert!(
        controller
            .reopen_session(runtrol_provider::SessionId::now(), &workspace)
            .is_err()
    );
    controller.reopen_session(session, &workspace).unwrap();
    assert!(!row(&scratch).session_completed);
    assert!(registry::update(&scratch.registry, completed).is_err());
    assert!(
        controller
            .release(&containment, Some(request), None, workspace.as_str())
            .await
            .is_err()
    );
    controller.complete_session(&ended).unwrap();
    let mut foreign = row(&scratch);
    let mut owner = ProcessScratch::start(&scratch);
    let keeper = current_process().into();
    foreign.lifetime = Some(GenerationProof {
        runtime: owner.identity.into(),
        keeper: Some(keeper),
        completed: false,
    });
    registry::update(&scratch.registry, foreign).unwrap();
    owner.stop();
    assert!(
        controller
            .release(&containment, Some(request), None, workspace.as_str())
            .await
            .is_err()
    );
    registry::complete(&scratch.registry, owner.identity.into(), keeper, || Ok(())).unwrap();
    controller
        .release(&containment, Some(request), None, workspace.as_str())
        .await
        .unwrap();
    assert!(!workspace.as_std_path().exists());
}

#[tokio::test]
async fn keeper_completion_preserves_commits_and_late_generation_cannot_complete_a_resumed_owner() {
    let scratch = Scratch::make();
    let (controller, ticket, keeper) = ended(&scratch).await;
    let original = row(&scratch);
    std::fs::write(
        original.workspace.as_std_path().join("work.txt"),
        b"retained work\n",
    )
    .unwrap();
    git(original.workspace.as_std_path(), &["add", "work.txt"]);
    git(
        original.workspace.as_std_path(),
        &["commit", "-m", "retained work"],
    );
    let committed = git_output(original.workspace.as_std_path(), &["rev-parse", "HEAD"]);
    registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Ok(())).unwrap();
    controller
        .recover_terminal(&Containment::without_any(), &ticket)
        .await
        .unwrap();
    assert_eq!(row(&scratch).state, State::PreservedDirty);
    assert_eq!(row(&scratch).base_commit, original.base_commit);
    assert_eq!(
        git_output(original.workspace.as_std_path(), &["rev-parse", "HEAD"]),
        committed
    );
    let binding = controller
        .resume_binding(&original.workspace)
        .unwrap()
        .unwrap();
    let owner = TerminalOwner {
        runtime: current_process().into(),
        terminal: TerminalId::now(),
    };
    let mut reservation = controller
        .reserve_resume(&Containment::without_any(), &binding, owner, |_| Ok(None))
        .unwrap();
    assert!(reservation.bind(None).is_err());
    drop(reservation);
    let mut pending = row(&scratch);
    pending
        .terminal
        .as_mut()
        .unwrap()
        .resume
        .as_mut()
        .unwrap()
        .lifetime = Some(GenerationProof {
        runtime: owner.runtime,
        keeper: Some(ticket.worker.runtime),
        completed: false,
    });
    registry::update(&scratch.registry, pending).unwrap();
    let before = row(&scratch).revision;
    registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Ok(())).unwrap();
    let after = row(&scratch);
    assert_eq!(after.revision, before);
    let resume = after.terminal.as_ref().unwrap().resume.as_ref().unwrap();
    assert!(!proven(resume.lifetime, owner.runtime));
    assert!(
        controller
            .recover_terminal(&Containment::without_any(), &ticket)
            .await
            .is_err()
    );
    assert!(original.workspace.as_std_path().join("work.txt").exists());
}

#[tokio::test]
async fn schema_three_upgrade_preserves_missing_proof_and_refuses_smuggled_completion() {
    let scratch = Scratch::make();
    let (controller, ticket, keeper) = ended(&scratch).await;
    let with_proof = row(&scratch);
    let path = registry::data_path(&scratch.registry).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&File {
            schema: 3,
            records: vec![with_proof.clone()],
        })
        .unwrap(),
    )
    .unwrap();
    assert!(registry::read(&scratch.registry).is_err());
    let mut legacy = with_proof;
    legacy.lifetime = None;
    std::fs::write(
        &path,
        serde_json::to_vec(&File {
            schema: 3,
            records: vec![legacy.clone()],
        })
        .unwrap(),
    )
    .unwrap();
    registry::check_writable(&scratch.registry).unwrap();
    let document: File = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(document.schema, FILE_SCHEMA);
    let migrated = document.records.first().unwrap();
    assert_eq!(migrated.revision, legacy.revision);
    assert_eq!(migrated.base_commit, legacy.base_commit);
    assert_eq!(migrated.workspace, legacy.workspace);
    assert_eq!(migrated.terminal.as_ref().unwrap().ticket, ticket);
    assert!(migrated.lifetime.is_none());
    registry::complete(&scratch.registry, ticket.worker.runtime, keeper, || Ok(())).unwrap();
    assert!(row(&scratch).lifetime.is_none());
    assert!(
        controller
            .recover_terminal(&Containment::without_any(), &ticket)
            .await
            .is_err()
    );
    assert!(legacy.workspace.as_std_path().exists());
}
