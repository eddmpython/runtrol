use super::process::{ProcessScratch, current_process};
use super::{Scratch, git, git_output};
use crate::isolated_workspace::ownership::{EndedSpawn, SpawnTicket, TerminalOwner};
use crate::isolated_workspace::{
    EndedResume, IsolatedWorkspaceController, VerifiedProject, WorktreeBinding, registry,
};
use runtrol_childproc::Containment;
use runtrol_provider::{AbsPath, ProcessIdentity, TerminalId};

fn occupant(runtime: ProcessIdentity) -> TerminalOwner {
    TerminalOwner {
        runtime: runtime.into(),
        terminal: TerminalId::now(),
    }
}

fn retire_original(
    binding: &WorktreeBinding,
    ticket: SpawnTicket,
    owner: TerminalOwner,
) -> Option<EndedResume> {
    (owner == ticket.worker).then(|| EndedResume::after_claim_retired(binding, owner))
}

async fn bound(
    scratch: &Scratch,
    controller: &mut IsolatedWorkspaceController,
    runtime: ProcessIdentity,
    worker: ProcessIdentity,
) -> (SpawnTicket, WorktreeBinding) {
    let ticket = SpawnTicket::new(runtime, TerminalId::now(), TerminalId::now(), 1).unwrap();
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    let prepared = controller
        .prepare_terminal(&Containment::without_any(), &ticket, &project)
        .await
        .unwrap();
    controller
        .bind_terminal(&ticket, worker, &prepared.workspace)
        .unwrap();
    let binding = controller
        .resume_binding(&prepared.workspace)
        .unwrap()
        .unwrap();
    (ticket, binding)
}

#[tokio::test]
async fn resumed_dirty_and_committed_work_preserves_original_binding_and_base() {
    for committed in [false, true] {
        let scratch = Scratch::make();
        let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
        let mut old_worker = ProcessScratch::start(&scratch);
        let (ticket, binding) = bound(
            &scratch,
            &mut controller,
            current_process(),
            old_worker.identity,
        )
        .await;
        old_worker.stop();
        std::fs::write(
            binding.workspace.as_std_path().join("work.txt"),
            b"owned work\n",
        )
        .unwrap();
        if committed {
            git(binding.workspace.as_std_path(), &["add", "work.txt"]);
            git(binding.workspace.as_std_path(), &["commit", "-m", "work"]);
        }
        let head = git_output(binding.workspace.as_std_path(), &["rev-parse", "HEAD"]);
        controller
            .release_terminal(
                &Containment::without_any(),
                &EndedSpawn::after_gate_retired(ticket),
            )
            .await
            .unwrap();
        let owner = occupant(current_process());
        let mut reservation = controller
            .reserve_resume(&binding, owner, |owner| {
                Ok(retire_original(&binding, ticket, owner))
            })
            .unwrap();
        let mut resumed = ProcessScratch::start(&scratch);
        reservation.bind(Some(resumed.identity)).unwrap();
        drop(reservation);
        let mut other = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
        assert!(
            other
                .release_terminal(
                    &Containment::without_any(),
                    &EndedSpawn::after_gate_retired(ticket)
                )
                .await
                .is_err()
        );
        let ended = EndedResume::after_terminal_exit(&binding, owner);
        assert!(
            other
                .release_resume(&Containment::without_any(), &ended)
                .await
                .is_err()
        );
        resumed.stop();
        other
            .release_resume(&Containment::without_any(), &ended)
            .await
            .unwrap();
        let rows = registry::read(&scratch.registry).unwrap();
        let row = rows.first().unwrap();
        assert_eq!(row.state, super::super::State::PreservedDirty);
        assert_eq!(row.base_commit, binding.base_commit);
        assert_eq!(row.terminal.as_ref().unwrap().ticket, ticket);
        assert!(row.terminal.as_ref().unwrap().resume.is_none());
        assert_eq!(
            git_output(binding.workspace.as_std_path(), &["rev-parse", "HEAD"]),
            head
        );
        assert_eq!(
            std::fs::read(binding.workspace.as_std_path().join("work.txt")).unwrap(),
            b"owned work\n"
        );
    }
}

#[tokio::test]
async fn live_original_and_pending_resume_exclude_other_controllers_without_blocking_binding_inspection()
 {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut runtime = ProcessScratch::start(&scratch);
    let mut worker = ProcessScratch::start(&scratch);
    let (ticket, binding) =
        bound(&scratch, &mut controller, runtime.identity, worker.identity).await;
    let owner = occupant(current_process());
    assert!(
        controller
            .resume_binding(&binding.workspace)
            .unwrap()
            .is_some(),
        "a viewer can inspect a live binding before native join"
    );
    assert!(
        controller
            .reserve_resume(&binding, owner, |owner| Ok(retire_original(
                &binding, ticket, owner
            )))
            .is_err()
    );
    worker.stop();
    runtime.stop();
    let mut reservation = controller
        .reserve_resume(&binding, owner, |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let mut other = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    assert!(
        other
            .reserve_resume(&binding, occupant(current_process()), |owner| Ok(
                retire_original(&binding, ticket, owner)
            ))
            .is_err()
    );
    assert!(
        other
            .recover_terminal(&Containment::without_any(), &ticket)
            .await
            .is_err()
    );
    // A post-birth identity failure must retain pending ownership after the operation lease is released.
    assert!(reservation.bind(None).is_err());
    drop(reservation);
    assert!(
        other
            .recover_terminal(&Containment::without_any(), &ticket)
            .await
            .is_err()
    );
    assert!(
        other
            .reserve_resume(&binding, occupant(current_process()), |owner| Ok(
                retire_original(&binding, ticket, owner)
            ))
            .is_err()
    );
    other
        .release_resume(
            &Containment::without_any(),
            &EndedResume::after_terminal_exit(&binding, owner),
        )
        .await
        .unwrap();
    assert!(
        !binding.workspace.as_std_path().exists(),
        "the exact exit observer can reclaim an unchanged worktree"
    );
}

#[tokio::test]
async fn cancellation_and_late_authority_refusal_leave_no_pending_occupant() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut worker = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        worker.identity,
    )
    .await;
    worker.stop();
    let before = std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap();
    assert!(
        controller
            .reserve_resume(&binding, occupant(current_process()), |_| Err(
                "revoked".to_owned()
            ))
            .is_err()
    );
    assert_eq!(
        std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap(),
        before
    );
    let reservation = controller
        .reserve_resume(&binding, occupant(current_process()), |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    drop(reservation);
    assert!(
        registry::read(&scratch.registry)
            .unwrap()
            .first()
            .unwrap()
            .terminal
            .as_ref()
            .unwrap()
            .resume
            .is_none()
    );
    let next = controller
        .reserve_resume(&binding, occupant(current_process()), |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    drop(next);
}

#[tokio::test]
async fn a_previous_resume_exit_cannot_clear_the_next_occupant() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let old_owner = occupant(current_process());
    let mut old = controller
        .reserve_resume(&binding, old_owner, |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let mut first = ProcessScratch::start(&scratch);
    old.bind(Some(first.identity)).unwrap();
    drop(old);
    first.stop();
    let new_owner = occupant(current_process());
    let mut next = controller
        .reserve_resume(&binding, new_owner, |owner| {
            Ok(Some(EndedResume::after_claim_retired(&binding, owner)))
        })
        .unwrap();
    let mut second = ProcessScratch::start(&scratch);
    next.bind(Some(second.identity)).unwrap();
    drop(next);
    controller
        .release_resume(
            &Containment::without_any(),
            &EndedResume::after_terminal_exit(&binding, old_owner),
        )
        .await
        .unwrap();
    assert_eq!(
        registry::read(&scratch.registry)
            .unwrap()
            .first()
            .unwrap()
            .terminal
            .as_ref()
            .unwrap()
            .resume
            .as_ref()
            .unwrap()
            .owner,
        new_owner
    );
    assert!(binding.workspace.as_std_path().exists());
    second.stop();
    controller
        .release_resume(
            &Containment::without_any(),
            &EndedResume::after_terminal_exit(&binding, new_owner),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn an_exact_bound_process_that_exits_before_the_observer_is_reclaimed() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let owner = occupant(current_process());
    let mut reservation = controller
        .reserve_resume(&binding, owner, |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let mut process = ProcessScratch::start(&scratch);
    let identity = process.identity;
    process.stop();
    reservation.bind(Some(identity)).unwrap();
    drop(reservation);
    controller
        .release_resume(
            &Containment::without_any(),
            &EndedResume::after_terminal_exit(&binding, owner),
        )
        .await
        .unwrap();
    assert!(!binding.workspace.as_std_path().exists());
}

#[tokio::test]
async fn unowned_siblings_and_replaced_filesystem_objects_cannot_supply_a_resume_binding() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let unknown = binding.workspace.parent().unwrap().join("unowned").unwrap();
    std::fs::create_dir(unknown.as_std_path()).unwrap();
    assert!(controller.resume_binding(&unknown).unwrap().is_none());
    let container = binding.workspace.parent().unwrap();
    for directory in [
        &scratch.project,
        &binding.project.store.path,
        &container,
        &binding.workspace,
    ] {
        let moved = directory.as_std_path().with_extension("held");
        std::fs::rename(directory.as_std_path(), &moved).unwrap();
        std::fs::create_dir(directory.as_std_path()).unwrap();
        assert!(controller.resume_binding(&binding.workspace).is_err());
        assert!(
            binding.verify().is_err(),
            "final birth must verify every captured filesystem object"
        );
        assert!(
            controller
                .reserve_resume(&binding, occupant(current_process()), |owner| Ok(
                    retire_original(&binding, ticket, owner)
                ))
                .is_err()
        );
        std::fs::remove_dir(directory.as_std_path()).unwrap();
        std::fs::rename(moved, directory.as_std_path()).unwrap();
    }
    let foreign = Scratch::make();
    let link = binding.workspace.as_std_path().join(".git");
    let original_link = std::fs::read(&link).unwrap();
    std::fs::write(
        &link,
        format!(
            "gitdir: {}\n",
            foreign.project.as_std_path().join(".git").display()
        ),
    )
    .unwrap();
    assert!(
        binding.verify().is_err(),
        "an unchanged directory cannot adopt another Git store"
    );
    assert!(controller.resume_binding(&binding.workspace).is_err());
    std::fs::write(&link, original_link).unwrap();
    let mut changed = binding.clone();
    changed.project = VerifiedProject::discover(&foreign.project).unwrap();
    assert!(
        controller
            .reserve_resume(&changed, occupant(current_process()), |owner| Ok(
                retire_original(&binding, ticket, owner)
            ))
            .is_err()
    );
    let unknown = AbsPath::canonicalize(unknown.as_str()).unwrap();
    assert!(controller.resume_binding(&unknown).unwrap().is_none());
}

#[tokio::test]
async fn schema_two_upgrade_preserves_committed_identity_and_excludes_stale_occupant_writes() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    std::fs::write(
        binding.workspace.as_std_path().join("committed.txt"),
        b"preserved commit\n",
    )
    .unwrap();
    git(binding.workspace.as_std_path(), &["add", "committed.txt"]);
    git(
        binding.workspace.as_std_path(),
        &["commit", "-m", "preserved work"],
    );
    let head = git_output(binding.workspace.as_std_path(), &["rev-parse", "HEAD"]);
    assert_ne!(
        String::from_utf8_lossy(&head).trim(),
        binding.base_commit.as_ref()
    );
    let path = registry::data_path(&scratch.registry).unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    *old.get_mut("schema").unwrap() = serde_json::json!(2);
    old.pointer_mut("/records/0/terminal")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("resume");
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let stale = registry::read(&scratch.registry).unwrap().remove(0);
    let reservation = controller
        .reserve_resume(&binding, occupant(current_process()), |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let new: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        new.get("schema").unwrap(),
        &serde_json::json!(super::super::FILE_SCHEMA)
    );
    for field in [
        "ticket",
        "root_identity",
        "store",
        "container",
        "directory",
        "process",
    ] {
        assert_eq!(
            new.pointer("/records/0/terminal")
                .unwrap()
                .get(field)
                .unwrap(),
            old.pointer("/records/0/terminal")
                .unwrap()
                .get(field)
                .unwrap()
        );
    }
    assert_eq!(
        new.pointer("/records/0/base_commit").unwrap(),
        old.pointer("/records/0/base_commit").unwrap()
    );
    assert!(registry::update(&scratch.registry, stale).is_err());
    drop(reservation);
    assert_eq!(
        git_output(binding.workspace.as_std_path(), &["rev-parse", "HEAD"]),
        head
    );
}

#[tokio::test]
async fn a_contended_cancellation_is_explicit_and_only_exact_current_owner_proof_recovers_it() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let owner = occupant(current_process());
    let reservation = controller
        .reserve_resume(&binding, owner, |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let mut writer = ProcessScratch::holding_registry(&scratch);
    assert!(
        reservation.abort().is_err(),
        "rollback contention must reach the caller"
    );
    writer.stop();
    let mut next = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    assert!(
        next.reserve_resume(&binding, occupant(current_process()), |owner| Ok(
            retire_original(&binding, ticket, owner)
        ))
        .is_err()
    );
    let wrong_terminal = occupant(current_process());
    assert!(
        next.reserve_resume(&binding, occupant(current_process()), |_| Ok(Some(
            EndedResume::after_claim_retired(&binding, wrong_terminal)
        )))
        .is_err()
    );
    let mut another_runtime = ProcessScratch::start(&scratch);
    assert!(
        next.reserve_resume(&binding, occupant(another_runtime.identity), |_| Ok(Some(
            EndedResume::after_claim_retired(&binding, owner)
        )))
        .is_err()
    );
    another_runtime.stop();
    assert!(
        next.reserve_resume(&binding, occupant(current_process()), |_| Err(
            "native claim state unavailable".to_owned()
        ))
        .is_err()
    );
    let recovered = next
        .reserve_resume(&binding, occupant(current_process()), |pending| {
            assert!(pending == ticket.worker || pending == owner);
            Ok(Some(EndedResume::after_claim_retired(&binding, pending)))
        })
        .unwrap();
    recovered.abort().unwrap();
    assert!(
        registry::read(&scratch.registry)
            .unwrap()
            .first()
            .unwrap()
            .terminal
            .as_ref()
            .unwrap()
            .resume
            .is_none()
    );
}

#[tokio::test]
async fn a_short_unrelated_writer_does_not_strand_a_prebirth_rollback() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let reservation = controller
        .reserve_resume(&binding, occupant(current_process()), |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    let mut writer = ProcessScratch::holding_registry(&scratch);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(25));
            writer.stop();
        });
        reservation.abort().unwrap();
    });
    assert!(
        registry::read(&scratch.registry)
            .unwrap()
            .first()
            .unwrap()
            .terminal
            .as_ref()
            .unwrap()
            .resume
            .is_none()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_reparse_workspace_cannot_reuse_the_captured_directory_binding() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (_, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let held = binding.workspace.as_std_path().with_extension("held");
    std::fs::rename(binding.workspace.as_std_path(), &held).unwrap();
    let made = std::os::windows::fs::symlink_dir(&held, binding.workspace.as_std_path());
    match made {
        Ok(()) => {
            assert!(binding.verify().is_err());
            assert!(controller.resume_binding(&binding.workspace).is_err());
            std::fs::remove_dir(binding.workspace.as_std_path()).unwrap();
        }
        Err(error) if error.raw_os_error() == Some(1314) => {}
        Err(error) => panic!("fixture link creation failed: {error}"),
    }
    std::fs::rename(held, binding.workspace.as_std_path()).unwrap();
}

#[tokio::test]
async fn an_ended_worker_in_another_live_runtime_keeps_its_worktree_until_runtime_retirement() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut runtime = ProcessScratch::start(&scratch);
    let mut worker = ProcessScratch::start(&scratch);
    let (ticket, binding) =
        bound(&scratch, &mut controller, runtime.identity, worker.identity).await;
    worker.stop();
    assert!(
        controller
            .resume_binding(&binding.workspace)
            .unwrap()
            .is_some()
    );
    assert!(
        controller
            .reserve_resume(&binding, occupant(current_process()), |owner| Ok(
                retire_original(&binding, ticket, owner)
            ))
            .is_err()
    );
    runtime.stop();
    controller
        .reserve_resume(&binding, occupant(current_process()), |_| Ok(None))
        .unwrap()
        .abort()
        .unwrap();
}

#[tokio::test]
async fn malformed_or_old_schema_occupants_never_become_resume_authority() {
    let scratch = Scratch::make();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut original = ProcessScratch::start(&scratch);
    let (ticket, binding) = bound(
        &scratch,
        &mut controller,
        current_process(),
        original.identity,
    )
    .await;
    original.stop();
    let mut reservation = controller
        .reserve_resume(&binding, occupant(current_process()), |owner| {
            Ok(retire_original(&binding, ticket, owner))
        })
        .unwrap();
    assert!(reservation.bind(None).is_err());
    drop(reservation);
    let path = registry::data_path(&scratch.registry).unwrap();
    let good: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (field, replacement) in [
        ("/schema", serde_json::json!(2)),
        (
            "/records/0/terminal/resume/owner",
            good.pointer("/records/0/terminal/ticket/worker")
                .unwrap()
                .clone(),
        ),
        ("/records/0/state", serde_json::json!("released")),
        (
            "/records/0/terminal/resume/process",
            serde_json::json!({"pid": 0, "started": 0}),
        ),
    ] {
        let mut value = good.clone();
        *value.pointer_mut(field).unwrap() = replacement;
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            IsolatedWorkspaceController::open(scratch.registry.clone()).is_err(),
            "invalid resume ownership must refuse"
        );
    }
}
