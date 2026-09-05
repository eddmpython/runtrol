//! Committed snapshots leave the source's index and uncommitted files intact.

use std::path::Path;

use runtrol_childproc::Containment;
use runtrol_ipc::wire::Response;
use runtrol_provider::TerminalId;

use super::process::{ProcessScratch, current_process};
use super::{Scratch, git, git_output};
use crate::isolated_workspace::ownership::{EndedSpawn, SpawnTicket};
use crate::isolated_workspace::{IsolatedWorkspaceController, VerifiedProject};

#[derive(PartialEq, Eq)]
pub(super) struct SourceSnapshot {
    commit: String,
    files: Vec<Vec<u8>>,
}

impl SourceSnapshot {
    pub(super) fn dirty(project: &Path) -> Self {
        std::fs::write(project.join("README.md"), b"staged content\n").unwrap();
        std::fs::write(project.join("staged.txt"), b"index-only file\n").unwrap();
        git(project, &["add", "README.md", "staged.txt"]);
        std::fs::write(project.join("README.md"), b"unstaged content\n").unwrap();
        std::fs::write(project.join("untracked.txt"), b"untracked content\n").unwrap();
        assert_eq!(
            git_output(project, &["show", ":README.md"]),
            b"staged content\n"
        );
        Self::capture(project)
    }

    fn capture(project: &Path) -> Self {
        Self {
            commit: head(project),
            files: [
                ".git/HEAD",
                ".git/index",
                "README.md",
                "staged.txt",
                "untracked.txt",
            ]
            .map(|name| std::fs::read(project.join(name)).unwrap())
            .into(),
        }
    }

    pub(super) fn assert_unchanged(&self, project: &Path) {
        assert!(
            *self == Self::capture(project),
            "the source HEAD, index and uncommitted files must remain byte-for-byte unchanged"
        );
    }

    fn assert_checkout(&self, workspace: &Path) {
        assert_eq!(head(workspace), self.commit);
        assert_eq!(
            std::fs::read_to_string(workspace.join("README.md"))
                .unwrap()
                .trim_end(),
            "base"
        );
        assert!(!workspace.join("staged.txt").exists());
        assert!(!workspace.join("untracked.txt").exists());
        assert!(
            git_output(
                workspace,
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all"
                ]
            )
            .is_empty(),
            "the new worktree contains only the frozen committed checkout"
        );
    }
}

fn head(project: &Path) -> String {
    String::from_utf8(git_output(
        project,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    ))
    .unwrap()
    .trim()
    .to_owned()
}

fn removed(response: Response) {
    let Response::IsolatedWorkspaceReleased(released) = response else {
        panic!("workspace release response");
    };
    assert_eq!(released.outcome.as_ref(), "removed");
}

#[tokio::test]
async fn ordinary_snapshot_preserves_dirty_source_and_retries_the_original_commit() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let request = "31234567-89ab-cdef-0123-456789abcdef";
    let source = SourceSnapshot::dirty(scratch.project.as_std_path());
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let Response::IsolatedWorkspace(first) = controller
        .prepare(&containment, request, scratch.project.as_str())
        .await
        .expect("create from committed HEAD while the source is dirty")
    else {
        panic!("workspace preparation response");
    };
    assert_eq!(first.base_commit.as_ref(), source.commit);
    source.assert_checkout(Path::new(first.workspace.as_ref()));
    source.assert_unchanged(scratch.project.as_std_path());

    // The operator advances only the already staged content. Unstaged and untracked work remain.
    git(
        scratch.project.as_std_path(),
        &["commit", "-m", "source advances"],
    );
    let advanced = SourceSnapshot::capture(scratch.project.as_std_path());
    assert_ne!(advanced.commit, source.commit);
    drop(controller);
    let mut reopened = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let Response::IsolatedWorkspace(retried) = reopened
        .prepare(&containment, request, scratch.project.as_str())
        .await
        .expect("retry the exact original request after restart and source advancement")
    else {
        panic!("workspace retry response");
    };
    assert_eq!(retried.workspace, first.workspace);
    assert_eq!(retried.base_commit, first.base_commit);
    source.assert_checkout(Path::new(retried.workspace.as_ref()));
    removed(
        reopened
            .release(&containment, Some(request), None, &retried.workspace)
            .await
            .unwrap(),
    );
    assert!(!Path::new(first.workspace.as_ref()).exists());
    advanced.assert_unchanged(scratch.project.as_std_path());
}

#[tokio::test]
async fn terminal_snapshot_keeps_its_base_and_live_owner_while_the_dirty_source_advances() {
    let scratch = Scratch::make();
    let containment = Containment::without_any();
    let source = SourceSnapshot::dirty(scratch.project.as_std_path());
    let project = VerifiedProject::discover(&scratch.project).unwrap();
    let ticket =
        SpawnTicket::new(current_process(), TerminalId::now(), TerminalId::now(), 1).unwrap();
    let mut controller = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let first = controller
        .prepare_terminal(&containment, &ticket, &project)
        .await
        .expect("create a terminal worktree from committed HEAD while the source is dirty");
    assert_eq!(first.base_commit.as_ref(), source.commit);
    source.assert_checkout(first.workspace.as_std_path());
    source.assert_unchanged(scratch.project.as_std_path());

    git(
        scratch.project.as_std_path(),
        &["commit", "-m", "source advances"],
    );
    let advanced = SourceSnapshot::capture(scratch.project.as_std_path());
    assert_ne!(advanced.commit, source.commit);
    drop(controller);
    let mut reopened = IsolatedWorkspaceController::open(scratch.registry.clone()).unwrap();
    let retried = reopened
        .prepare_terminal(&containment, &ticket, &project)
        .await
        .expect("retry the exact terminal reservation after restart and source advancement");
    assert_eq!(retried.workspace, first.workspace);
    assert_eq!(retried.base_commit, first.base_commit);
    source.assert_checkout(retried.workspace.as_std_path());

    let mut worker = ProcessScratch::start(&scratch);
    reopened
        .bind_terminal(&ticket, worker.identity, &retried.workspace)
        .unwrap();
    let ended = EndedSpawn::after_gate_retired(ticket);
    assert!(
        reopened
            .release_terminal(&containment, &ended)
            .await
            .is_err(),
        "even an ended permit cannot remove the exact worker while its process remains live"
    );
    assert!(retried.workspace.as_std_path().exists());
    worker.stop();
    removed(
        reopened
            .release_terminal(&containment, &ended)
            .await
            .unwrap(),
    );
    assert!(!retried.workspace.as_std_path().exists());
    advanced.assert_unchanged(scratch.project.as_std_path());
}
