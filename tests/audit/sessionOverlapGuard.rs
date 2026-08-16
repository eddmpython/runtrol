//! Hosted proof that overlapping agent writers are admitted atomically by the Core.

use std::path::PathBuf;

use async_trait::async_trait;
use runtrol_core::{ProjectIdentity, SessionError, SessionManager, WorkspaceClaim};
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, CloseMode, Disposition, OpenIntent, Produced, ProviderError,
    ProviderId, SessionId, WorkspaceAccess,
};

struct IdleAgent(SessionId);

#[async_trait]
impl Agent for IdleAgent {
    fn session(&self) -> SessionId {
        self.0
    }

    fn native(&self) -> Option<&str> {
        None
    }

    async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        core::future::pending().await
    }

    async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "runtrol-session-overlap-{name}-{}",
            std::process::id(),
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous audit directory");
        }
        std::fs::create_dir_all(&root).expect("create the audit directory");
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn canonical(&self, relative: &str) -> AbsPath {
        AbsPath::canonicalize(
            self.path(relative)
                .to_str()
                .expect("the temporary path is UTF-8"),
        )
        .expect("the audit workspace is canonical")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::NotFound,
                "remove the audit directory: {error}"
            );
        }
    }
}

fn claim(workspace: AbsPath, access: WorkspaceAccess) -> WorkspaceClaim {
    WorkspaceClaim::discover(workspace, access).expect("resolve the project identity")
}

fn intent(session: SessionId, workspace: AbsPath) -> OpenIntent {
    OpenIntent {
        session,
        workspace,
        disposition: Disposition::Fresh,
        model: None,
        reasoning_effort: None,
        permission: None,
    }
}

#[test]
fn opening_live_and_closing_claims_all_block_a_second_writer() {
    let scratch = Scratch::new("lifecycle");
    std::fs::create_dir_all(scratch.path("repo/.git")).expect("create repository metadata");
    std::fs::create_dir_all(scratch.path("repo/frontend")).expect("create first workspace");
    std::fs::create_dir_all(scratch.path("repo/backend")).expect("create second workspace");

    let first_workspace = scratch.canonical("repo/frontend");
    let second_workspace = scratch.canonical("repo/backend");
    let first = SessionId::now();
    let second = SessionId::now();
    let mut sessions = SessionManager::new();
    let opening = sessions
        .reserve_open(
            first,
            claim(first_workspace.clone(), WorkspaceAccess::Exclusive),
        )
        .expect("reserve the first writer");

    assert_busy(&mut sessions, second, second_workspace.clone(), first);

    sessions
        .attach_opened(
            opening.reservation,
            ProviderId::parse("fixture").expect("valid provider"),
            &intent(first, first_workspace),
            Box::new(IdleAgent(first)),
        )
        .expect("attach the first writer");
    assert_busy(&mut sessions, second, second_workspace.clone(), first);

    let closing = sessions.close(first).expect("begin process cleanup");
    assert_busy(&mut sessions, second, second_workspace.clone(), first);

    sessions.release_closing(closing.reservation);
    assert!(
        sessions
            .reserve_open(second, claim(second_workspace, WorkspaceAccess::Exclusive),)
            .is_ok(),
        "only exact cleanup releases the writer identity"
    );
}

#[test]
fn linked_worktrees_are_isolated_but_sharing_one_worktree_requires_explicit_consent() {
    let scratch = Scratch::new("linked");
    std::fs::create_dir_all(scratch.path("main/.git/worktrees/linked"))
        .expect("create linked repository metadata");
    std::fs::create_dir_all(scratch.path("main/src")).expect("create main workspace");
    std::fs::create_dir_all(scratch.path("linked/src")).expect("create linked workspace");
    std::fs::write(
        scratch.path("linked/.git"),
        format!(
            "gitdir: {}\n",
            scratch
                .path("main/.git/worktrees/linked")
                .to_str()
                .expect("the temporary path is UTF-8")
        ),
    )
    .expect("write the linked gitdir pointer");
    std::fs::write(
        scratch.path("main/.git/worktrees/linked/commondir"),
        "../..\n",
    )
    .expect("write the common repository pointer");

    let main = ProjectIdentity::discover(scratch.canonical("main/src")).expect("discover main");
    let linked =
        ProjectIdentity::discover(scratch.canonical("linked/src")).expect("discover linked");
    assert_eq!(main.common_store(), linked.common_store());
    assert!(!main.overlaps(&linked));

    let mut sessions = SessionManager::new();
    sessions
        .reserve_open(
            SessionId::now(),
            WorkspaceClaim::new(main.clone(), WorkspaceAccess::Exclusive),
        )
        .expect("reserve the main worktree");
    sessions
        .reserve_open(
            SessionId::now(),
            WorkspaceClaim::new(linked, WorkspaceAccess::Exclusive),
        )
        .expect("a linked worktree is an independent writer identity");
    sessions
        .reserve_open(
            SessionId::now(),
            WorkspaceClaim::new(main, WorkspaceAccess::Shared),
        )
        .expect("explicit consent is the only same-worktree override");
}

fn assert_busy(
    sessions: &mut SessionManager,
    requested: SessionId,
    workspace: AbsPath,
    occupied_by: SessionId,
) {
    assert!(matches!(
        sessions.reserve_open(requested, claim(workspace, WorkspaceAccess::Exclusive)),
        Err(SessionError::WorkspaceOccupied { session, .. }) if session == occupied_by
    ));
}
