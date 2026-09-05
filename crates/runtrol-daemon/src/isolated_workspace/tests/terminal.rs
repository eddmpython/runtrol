use super::process::{ProcessScratch, current_process};
use super::{Scratch, git};
use crate::isolated_workspace::ownership::{EndedSpawn, SpawnTicket};
use crate::isolated_workspace::{IsolatedWorkspaceController, PreparedWorkspace, VerifiedProject};
use runtrol_childproc::Containment;
use runtrol_ipc::wire::Response;
use runtrol_provider::{ProcessIdentity, SessionId, TerminalId};

fn ticket(runtime: ProcessIdentity) -> SpawnTicket {
    SpawnTicket::new(runtime, TerminalId::now(), TerminalId::now(), 1).unwrap()
}

async fn prepare(
    scratch: &Scratch,
    controller: &mut IsolatedWorkspaceController,
    owner: &SpawnTicket,
) -> PreparedWorkspace {
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    controller
        .prepare_terminal(&Containment::without_any(), owner, &project)
        .await
        .unwrap()
}

fn outcome(response: Response) -> Box<str> {
    let Response::IsolatedWorkspaceReleased(line) = response else {
        panic!("release response")
    };
    line.outcome
}

#[tokio::test]
async fn terminal_pending_and_live_rows_refuse_legacy_mutation_and_preserve_lead_independence() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let owner = ticket(current_process());
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    assert!(
        controller
            .release_terminal_if_present(&containment, &EndedSpawn::after_gate_retired(owner))
            .await
            .unwrap()
            .is_none()
    );
    let prepared = prepare(&scratch, &mut controller, &owner).await;
    assert!(matches!(prepared.base_commit.len(), 40 | 64));
    assert_eq!(
        prepared.workspace_identity,
        runtrol_security::ProjectRootIdentity::read(&prepared.workspace)
            .unwrap()
            .to_bytes()
    );
    let Response::IsolatedWorkspaces(legacy_list) = controller.list() else {
        panic!("legacy list")
    };
    assert!(
        legacy_list.is_empty(),
        "terminal worktrees are projected only through TerminalDescriptor"
    );
    assert!(
        controller
            .release(
                &containment,
                Some(&owner.reservation_id()),
                None,
                prepared.workspace.as_str()
            )
            .await
            .is_err()
    );
    assert!(
        controller
            .bind(
                &owner.reservation_id(),
                &SessionId::now().to_string(),
                prepared.workspace.as_str()
            )
            .is_err()
    );
    assert!(
        controller
            .prepare(
                &containment,
                &owner.reservation_id(),
                scratch.project.as_str()
            )
            .await
            .is_err()
    );
    let mut worker = ProcessScratch::start(&scratch);
    controller
        .bind_terminal(&owner, worker.identity, &prepared.workspace)
        .unwrap();
    controller
        .bind_terminal(&owner, worker.identity, &prepared.workspace)
        .unwrap();
    let ended = EndedSpawn::after_gate_retired(owner);
    assert!(
        controller
            .release_terminal(&containment, &ended)
            .await
            .is_err()
    );
    assert!(
        controller
            .recover_terminal(&containment, &owner)
            .await
            .is_err()
    );
    let unrelated = ticket(current_process());
    assert!(
        controller
            .release_terminal(&containment, &EndedSpawn::after_gate_retired(unrelated))
            .await
            .is_err()
    );
    worker.stop();
    assert_eq!(
        outcome(
            controller
                .release_terminal(&containment, &ended)
                .await
                .unwrap()
        )
        .as_ref(),
        "removed"
    );
    assert!(!prepared.workspace.as_std_path().exists());
}

#[tokio::test]
async fn exact_old_runtime_and_worker_exit_enable_recovery_across_reopen() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let mut runtime = ProcessScratch::start(&scratch);
    let mut worker = ProcessScratch::start(&scratch);
    let owner = ticket(runtime.identity);
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let prepared = prepare(&scratch, &mut controller, &owner).await;
    controller
        .bind_terminal(&owner, worker.identity, &prepared.workspace)
        .unwrap();
    drop(controller);
    let mut reopened = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    reopened.recover_ended(&containment).await.unwrap();
    assert!(prepared.workspace.as_std_path().exists());
    assert!(
        reopened
            .recover_terminal(&containment, &owner)
            .await
            .is_err()
    );
    let mut wrong = owner;
    wrong.worker.runtime = current_process().into();
    assert!(
        reopened
            .recover_terminal(&containment, &wrong)
            .await
            .is_err()
    );
    runtime.stop();
    reopened.recover_ended(&containment).await.unwrap();
    assert!(prepared.workspace.as_std_path().exists());
    assert!(
        reopened
            .recover_terminal(&containment, &owner)
            .await
            .is_err()
    );
    worker.stop();
    reopened.recover_ended(&containment).await.unwrap();
    assert_eq!(
        outcome(
            reopened
                .recover_terminal(&containment, &owner)
                .await
                .unwrap()
        )
        .as_ref(),
        "alreadyRemoved"
    );
    assert!(!prepared.workspace.as_std_path().exists());
}

#[tokio::test]
async fn dirty_and_committed_worker_work_survive_cleanup_and_restart() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let source = super::snapshot::SourceSnapshot::dirty(scratch.project.as_std_path());
    for commit in [false, true] {
        let owner = ticket(current_process());
        let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
        let prepared = prepare(&scratch, &mut controller, &owner).await;
        let change = prepared.workspace.as_std_path().join("worker.txt");
        std::fs::write(&change, "작업 보존\n").unwrap();
        if commit {
            git(prepared.workspace.as_std_path(), &["add", "worker.txt"]);
            git(
                prepared.workspace.as_std_path(),
                &["commit", "-m", "worker"],
            );
        }
        let mut reopened = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
        assert_eq!(
            outcome(
                reopened
                    .release_terminal(&containment, &EndedSpawn::after_gate_retired(owner))
                    .await
                    .unwrap()
            )
            .as_ref(),
            "preservedDirty"
        );
        assert_eq!(std::fs::read_to_string(change).unwrap(), "작업 보존\n");
        source.assert_unchanged(scratch.project.as_std_path());
    }
}

#[tokio::test]
async fn creation_failure_and_late_cancel_remove_only_exact_reserved_worktree() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let owner = ticket(current_process());
    let hook = scratch
        .project
        .as_std_path()
        .join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    assert!(
        controller
            .prepare_terminal(&containment, &owner, &project)
            .await
            .is_err()
    );
    let owned_path = scratch
        .root
        .join(".runtrol-worktrees")
        .join(format!("chat-{}", owner.reservation_id()));
    assert!(owned_path.join(".git").is_file());
    std::fs::remove_file(hook).unwrap();
    let fresh = ticket(current_process());
    let fresh_workspace = prepare(&scratch, &mut controller, &fresh).await;
    assert_eq!(
        outcome(
            controller
                .release_terminal(&containment, &EndedSpawn::after_gate_retired(owner))
                .await
                .unwrap()
        )
        .as_ref(),
        "removed"
    );
    assert!(!owned_path.exists());
    assert!(fresh_workspace.workspace.as_std_path().is_dir());
    assert_eq!(
        outcome(
            controller
                .release_terminal(&containment, &EndedSpawn::after_gate_retired(fresh))
                .await
                .unwrap()
        )
        .as_ref(),
        "removed"
    );
}

#[tokio::test]
async fn replaced_root_and_worktree_objects_are_refused_without_touching_replacement() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let owner = ticket(current_process());
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let prepared = prepare(&scratch, &mut controller, &owner).await;
    let moved = scratch.root.join("original-project");
    std::fs::rename(scratch.project.as_std_path(), &moved).unwrap();
    std::fs::create_dir(scratch.project.as_std_path()).unwrap();
    std::fs::write(scratch.project.as_std_path().join("keep"), "replacement").unwrap();
    assert!(
        controller
            .bind_terminal(&owner, current_process(), &prepared.workspace)
            .is_err()
    );
    let ended = EndedSpawn::after_gate_retired(owner);
    assert!(
        controller
            .release_terminal(&containment, &ended)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(scratch.project.as_std_path().join("keep")).unwrap(),
        "replacement"
    );
    std::fs::remove_file(scratch.project.as_std_path().join("keep")).unwrap();
    std::fs::remove_dir(scratch.project.as_std_path()).unwrap();
    std::fs::rename(moved, scratch.project.as_std_path()).unwrap();
    let moved = scratch.root.join("original-worker");
    std::fs::rename(prepared.workspace.as_std_path(), &moved).unwrap();
    std::fs::create_dir(prepared.workspace.as_std_path()).unwrap();
    std::fs::copy(
        moved.join(".git"),
        prepared.workspace.as_std_path().join(".git"),
    )
    .unwrap();
    assert!(
        controller
            .release_terminal(&containment, &ended)
            .await
            .is_err()
    );
    std::fs::remove_file(prepared.workspace.as_std_path().join(".git")).unwrap();
    std::fs::remove_dir(prepared.workspace.as_std_path()).unwrap();
    std::fs::rename(moved, prepared.workspace.as_std_path()).unwrap();
    assert_eq!(
        outcome(
            controller
                .release_terminal(&containment, &ended)
                .await
                .unwrap()
        )
        .as_ref(),
        "removed"
    );
}
