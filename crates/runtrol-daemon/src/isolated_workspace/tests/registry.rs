use super::{
    Scratch,
    process::{ProcessScratch, current_process},
};
use crate::isolated_workspace::{
    IsolatedWorkspaceController, VerifiedProject,
    ownership::{EndedSpawn, SpawnTicket},
    registry,
};
use runtrol_childproc::Containment;
use runtrol_ipc::wire::Response;
use runtrol_provider::{SessionId, TerminalId};

#[cfg(windows)]
#[test]
fn interrupted_migration_reuses_only_its_exact_staging_document() {
    for staged_name in [None, Some("registry.json"), Some("keep.txt")] {
        let scratch = Scratch::make();
        std::fs::remove_file(registry::data_path(&scratch.registry).unwrap()).unwrap();
        std::fs::remove_dir(scratch.registry.as_std_path()).unwrap();
        let legacy = br#"{"schema":1,"records":[]}"#;
        std::fs::write(scratch.registry.as_std_path(), legacy).unwrap();
        let staged = scratch.registry.as_std_path().with_extension("migrating");
        std::fs::create_dir(&staged).unwrap();
        if let Some(name) = staged_name {
            std::fs::write(staged.join(name), b"interrupted").unwrap();
        }
        let result = registry::check_writable(&scratch.registry);
        if staged_name == Some("keep.txt") {
            assert!(result.unwrap_err().contains("unknown entry"));
            assert_eq!(
                std::fs::read(staged.join("keep.txt")).unwrap(),
                b"interrupted"
            );
            assert_eq!(
                std::fs::read(scratch.registry.as_std_path()).unwrap(),
                legacy
            );
        } else {
            result.unwrap();
            assert!(scratch.registry.as_std_path().is_dir());
            assert!(!staged.exists());
            assert!(registry::read(&scratch.registry).unwrap().is_empty());
        }
    }
}

#[test]
fn a_container_without_its_document_cannot_be_reinitialized_as_empty() {
    let scratch = Scratch::make();
    let document = registry::data_path(&scratch.registry).unwrap();
    std::fs::remove_file(&document).unwrap();
    assert!(
        registry::read(&scratch.registry)
            .unwrap_err()
            .contains("document is missing")
    );
    assert!(registry::check_writable(&scratch.registry).is_err());
    assert!(!document.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn schema_one_migration_preserves_unverified_ownership_and_excludes_cached_writers() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let id = "01234567-89ab-cdef-0123-456789abcdef";
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let Response::IsolatedWorkspace(workspace) = controller
        .prepare(&containment, id, scratch.project.as_str())
        .await
        .unwrap()
    else {
        panic!("prepared")
    };
    let session = SessionId::now().to_string();
    controller.bind(id, &session, &workspace.workspace).unwrap();
    let current = registry::data_path(&scratch.registry).unwrap();
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&current).unwrap()).unwrap();
    *file.get_mut("schema").unwrap() = serde_json::json!(1);
    for record in file.get_mut("records").unwrap().as_array_mut().unwrap() {
        for field in ["revision", "terminal", "legacy"] {
            record.as_object_mut().unwrap().remove(field);
        }
    }
    std::fs::remove_file(current).unwrap();
    std::fs::remove_dir(scratch.registry.as_std_path()).unwrap();
    std::fs::write(
        scratch.registry.as_std_path(),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    let mut old_writer = ProcessScratch::waiting_legacy_commit(&scratch);
    let mut reopened = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let Response::IsolatedWorkspaces(listed) = reopened.list() else {
        panic!("legacy list")
    };
    let listed = listed.first().unwrap();
    assert_eq!(listed.session_id.as_deref(), Some(session.as_str()));
    assert_eq!(listed.state.as_ref(), "bound");
    assert!(
        reopened
            .release(&containment, Some(id), None, &workspace.workspace)
            .await
            .unwrap_err()
            .contains("legacy worktree ownership")
    );
    assert!(std::path::Path::new(workspace.workspace.as_ref()).is_dir());
    assert!(scratch.registry.as_std_path().is_dir());
    let published = std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap();
    old_writer.stop();
    assert_eq!(
        std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap(),
        published
    );
    let records = registry::read(&scratch.registry).unwrap();
    let record = records.first().unwrap();
    assert!(record.legacy);
    assert_eq!(record.session_id.as_deref(), Some(session.as_str()));
    assert_eq!(record.workspace_id.as_ref(), id);
}

#[cfg(windows)]
#[test]
fn a_live_legacy_file_writer_delays_migration_without_changing_committed_metadata() {
    let scratch = Scratch::make();
    let current = registry::data_path(&scratch.registry).unwrap();
    std::fs::remove_file(current).unwrap();
    std::fs::remove_dir(scratch.registry.as_std_path()).unwrap();
    let legacy = br#"{"schema":1,"records":[]}"#;
    std::fs::write(scratch.registry.as_std_path(), legacy).unwrap();
    let temporary = scratch
        .registry
        .as_std_path()
        .with_file_name("isolated-workspaces.json.writing");
    let writing = std::fs::File::create(&temporary).unwrap();
    assert!(
        registry::check_writable(&scratch.registry)
            .unwrap_err()
            .contains("legacy worktree commit is busy")
    );
    assert_eq!(
        std::fs::read(scratch.registry.as_std_path()).unwrap(),
        legacy
    );
    drop(writing);
    registry::check_writable(&scratch.registry).unwrap();
    let published = std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap();
    registry::check_writable(&scratch.registry).unwrap();
    assert_eq!(
        published,
        std::fs::read(registry::data_path(&scratch.registry).unwrap()).unwrap()
    );
    assert!(std::fs::rename(temporary, scratch.registry.as_std_path()).is_err());
}

#[tokio::test]
async fn stale_controllers_preserve_other_generation_and_reject_stale_row_revision() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let old_runtime = ProcessScratch::start(&scratch);
    let new_runtime = ProcessScratch::start(&scratch);
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    let one = SpawnTicket::new(
        old_runtime.identity,
        TerminalId::now(),
        TerminalId::now(),
        1,
    )
    .unwrap();
    let two = SpawnTicket::new(
        new_runtime.identity,
        TerminalId::now(),
        TerminalId::now(),
        1,
    )
    .unwrap();
    let mut old = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let mut new = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let first = old
        .prepare_terminal(&containment, &one, &project)
        .await
        .unwrap();
    let stale = registry::read(&scratch.registry).unwrap().remove(0);
    let second = new
        .prepare_terminal(&containment, &two, &project)
        .await
        .unwrap();
    assert_eq!(registry::read(&scratch.registry).unwrap().len(), 2);
    old.bind_terminal(&one, old_runtime.identity, &first.workspace)
        .unwrap();
    assert!(new.recover_terminal(&containment, &one).await.is_err());
    assert!(
        registry::update(&scratch.registry, stale)
            .unwrap_err()
            .contains("revision changed")
    );
    new.release_terminal(&containment, &EndedSpawn::after_gate_retired(two))
        .await
        .unwrap();
    assert!(first.workspace.as_std_path().is_dir());
    assert!(!second.workspace.as_std_path().exists());
    let rows = registry::read(&scratch.registry).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|row| row.workspace_id.as_ref() == one.reservation_id())
    );
}

#[tokio::test]
async fn another_process_native_lock_refuses_and_releases_after_exact_exit() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    let owner =
        SpawnTicket::new(current_process(), TerminalId::now(), TerminalId::now(), 1).unwrap();
    let mut holder = ProcessScratch::holding_registry(&scratch);
    assert!(
        controller
            .prepare_terminal(&containment, &owner, &project)
            .await
            .unwrap_err()
            .contains("busy")
    );
    assert!(registry::read(&scratch.registry).unwrap().is_empty());
    holder.stop();
    let prepared = controller
        .prepare_terminal(&containment, &owner, &project)
        .await
        .unwrap();
    let held = registry::operation(&scratch.registry, &owner.reservation_id()).unwrap();
    assert!(
        controller
            .release_terminal(&containment, &EndedSpawn::after_gate_retired(owner))
            .await
            .unwrap_err()
            .contains("busy")
    );
    drop(held);
    controller
        .release_terminal(&containment, &EndedSpawn::after_gate_retired(owner))
        .await
        .unwrap();
    assert!(!prepared.workspace.as_std_path().exists());
}
