use super::*;
#[cfg(windows)]
mod cancellation;
mod process;
mod registry;
mod snapshot;
mod terminal;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    root: std::path::PathBuf,
    shared: std::path::PathBuf,
    project: AbsPath,
    registry: AbsPath,
}

impl Scratch {
    fn make() -> Self {
        let shared = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| {
                std::env::var_os("LOCALAPPDATA")
                    .map_or_else(std::env::temp_dir, std::path::PathBuf::from)
                    .join("dev-workspace")
            });
        std::fs::create_dir_all(&shared).expect("shared execution root");
        let root = shared.join(format!(
            "runtrol-isolated-workspace-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("project")).expect("create project");
        std::fs::write(root.join("fixture-owner"), std::process::id().to_string())
            .expect("fixture owner");
        std::fs::create_dir_all(root.join("home")).expect("create home");
        git(&root.join("project"), &["init"]);
        git(
            &root.join("project"),
            &["config", "user.email", "fixture@example.invalid"],
        );
        git(&root.join("project"), &["config", "user.name", "Fixture"]);
        std::fs::write(root.join("project/README.md"), b"base\n").expect("write base file");
        git(&root.join("project"), &["add", "README.md"]);
        git(&root.join("project"), &["commit", "-m", "fixture"]);
        let fixture = Self {
            project: AbsPath::canonicalize(root.join("project").to_string_lossy().as_ref())
                .expect("canonical project"),
            registry: AbsPath::canonicalize(root.join("home").to_string_lossy().as_ref())
                .expect("canonical home")
                .join("isolated-workspaces.json")
                .expect("registry path"),
            shared,
            root,
        };
        super::registry::check_writable(&fixture.registry)
            .expect("publish the fresh owned registry container");
        fixture
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.root.exists() {
            let exact = std::fs::canonicalize(&self.root).expect("canonical owned scratch");
            let shared =
                std::fs::canonicalize(&self.shared).expect("canonical shared execution root");
            assert_eq!(exact.parent(), Some(shared.as_path()));
            assert!(
                exact
                    .file_name()
                    .expect("fixture name")
                    .to_string_lossy()
                    .starts_with("runtrol-isolated-workspace-")
            );
            assert_eq!(
                std::fs::read_to_string(exact.join("fixture-owner")).expect("read fixture owner"),
                std::process::id().to_string()
            );
            std::fs::remove_dir_all(exact).expect("remove owned scratch tree");
        }
    }
}

fn git(project: &std::path::Path, arguments: &[&str]) {
    drop(git_output(project, arguments));
}

fn git_output(project: &std::path::Path, arguments: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(arguments)
        .output()
        .expect("run Git fixture command");
    assert!(
        output.status.success(),
        "Git fixture command failed: {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn durable_registry_rejects_targets_outside_the_owned_root() {
    let root = AbsPath::canonicalize(std::env::temp_dir().to_string_lossy().as_ref())
        .expect("canonical temporary directory");
    let project = root.join("project").expect("project path");
    let record = Record {
        workspace_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
        request_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
        project,
        workspace: root.join("somewhere-else").expect("outside path"),
        base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        session_id: None,
        state: State::Ready,
        revision: 1,
        terminal: None,
        legacy: false,
    };
    assert!(
        validate_records(&[record])
            .expect_err("outside target refused")
            .contains("outside the owned root")
    );
}

#[test]
fn identifiers_are_canonical_lowercase_uuids() {
    assert!(validate_uuid("01234567-89ab-cdef-0123-456789abcdef", "fixture").is_ok());
    assert!(validate_uuid("01234567-89AB-cdef-0123-456789abcdef", "fixture").is_err());
    assert!(validate_uuid("../escape", "fixture").is_err());
}

#[tokio::test]
async fn restart_preserves_session_binding_and_removes_the_exact_clean_worktree() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let first_id = "01234567-89ab-cdef-0123-456789abcdef";
    let mut controller =
        IsolatedWorkspaceController::open(scratch.registry.clone()).expect("open registry");
    let Response::IsolatedWorkspace(first) = controller
        .prepare(&containment, first_id, scratch.project.as_str())
        .await
        .expect("prepare first worktree")
    else {
        panic!("prepared response");
    };
    assert_ne!(first.workspace.as_ref(), scratch.project.as_str());
    let first_path = first.workspace.to_string();
    drop(controller);

    let mut restored =
        IsolatedWorkspaceController::open(scratch.registry.clone()).expect("restore registry");
    let Response::IsolatedWorkspaces(listed) = restored.list() else {
        panic!("listed response");
    };
    assert_eq!(listed.len(), 1);
    let session = SessionId::now().to_string();
    restored
        .bind(first_id, &session, &first_path)
        .expect("bind restored worktree");
    let Response::IsolatedWorkspaceReleased(released) = restored
        .release(&containment, None, Some(&session), &first_path)
        .await
        .expect("release clean worktree")
    else {
        panic!("release response");
    };
    assert_eq!(released.outcome.as_ref(), "removed");
    assert!(!std::path::Path::new(&first_path).exists());
}

#[tokio::test]
async fn cleanup_preserves_changes_across_restart() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let second_id = "11234567-89ab-cdef-0123-456789abcdef";
    let mut restored =
        IsolatedWorkspaceController::open(scratch.registry.clone()).expect("open registry");
    let Response::IsolatedWorkspace(second) = restored
        .prepare(&containment, second_id, scratch.project.as_str())
        .await
        .expect("prepare second worktree")
    else {
        panic!("prepared response");
    };
    let dirty = std::path::Path::new(second.workspace.as_ref()).join("agent-change.txt");
    std::fs::write(&dirty, b"keep me\n").expect("write agent change");
    let Response::IsolatedWorkspaceReleased(preserved) = restored
        .release(
            &containment,
            Some(second.workspace_id.as_ref()),
            None,
            second.workspace.as_ref(),
        )
        .await
        .expect("preserve dirty worktree")
    else {
        panic!("preserve response");
    };
    assert_eq!(preserved.outcome.as_ref(), "preservedDirty");
    assert!(dirty.exists());
    drop(restored);

    let mut after_restart =
        IsolatedWorkspaceController::open(scratch.registry.clone()).expect("restore dirty record");
    let Response::IsolatedWorkspaces(listed) = after_restart.list() else {
        panic!("listed response");
    };
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed
            .first()
            .expect("one retained workspace")
            .state
            .as_ref(),
        "preservedDirty"
    );
    std::fs::remove_file(&dirty).expect("clean retained worktree");
    let Response::IsolatedWorkspaceReleased(removed) = after_restart
        .release(
            &containment,
            Some(second.workspace_id.as_ref()),
            None,
            second.workspace.as_ref(),
        )
        .await
        .expect("remove cleaned worktree")
    else {
        panic!("removed response");
    };
    assert_eq!(removed.outcome.as_ref(), "removed");
    assert!(!std::path::Path::new(second.workspace.as_ref()).exists());
}
