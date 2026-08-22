//! Core-owned linked worktrees for ordinary parallel chats.
//!
//! Studio asks for isolation but never creates, guesses, or deletes a worktree. This controller freezes the
//! selected checkout's clean `HEAD`, records ownership before asking Git to create anything, binds the result to
//! the public Runtime session, and removes only an exact owned worktree that Git proves is clean. Conversation
//! bytes, provider identities, branches, commits, and integration policy are deliberately absent.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::time::Duration;

use runtrol_childproc::{Containment, capture, resolve};
use runtrol_core::project::ProjectIdentity;
use runtrol_ipc::wire::{IsolatedWorkspaceLine, IsolatedWorkspaceReleaseLine, Response};
use runtrol_provider::{AbsPath, SessionId};
use serde::{Deserialize, Serialize};

const FILE_SCHEMA: u8 = 1;
const MAX_RECORDS: usize = 128;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const GIT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(15);
const GIT_WORKTREE_TIMEOUT: Duration = Duration::from_mins(1);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum State {
    Creating,
    Ready,
    Bound,
    PreservedDirty,
    Released,
}

impl State {
    const fn wire(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Bound => "bound",
            Self::PreservedDirty => "preservedDirty",
            Self::Released => "released",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    workspace_id: Box<str>,
    request_id: Box<str>,
    project: AbsPath,
    workspace: AbsPath,
    base_commit: Box<str>,
    session_id: Option<Box<str>>,
    state: State,
}

impl Record {
    fn line(&self) -> IsolatedWorkspaceLine {
        IsolatedWorkspaceLine {
            workspace_id: self.workspace_id.clone(),
            project: self.project.as_str().into(),
            workspace: self.workspace.as_str().into(),
            base_commit: self.base_commit.clone(),
            state: self.state.wire().into(),
            session_id: self.session_id.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct File {
    schema: u8,
    records: Vec<Record>,
}

/// Durable owner of ordinary-chat linked worktrees.
pub(crate) struct IsolatedWorkspaceController {
    path: AbsPath,
    records: Vec<Record>,
}

impl IsolatedWorkspaceController {
    pub(crate) fn open(path: AbsPath) -> Result<Self, String> {
        let records = match std::fs::read(path.as_std_path()) {
            Ok(bytes) => {
                if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_FILE_BYTES) {
                    return Err(
                        "the isolated workspace registry exceeds its fixed bound".to_owned()
                    );
                }
                let file: File = serde_json::from_slice(&bytes)
                    .map_err(|_| "the isolated workspace registry is malformed".to_owned())?;
                if file.schema != FILE_SCHEMA {
                    return Err("the isolated workspace registry schema is unsupported".to_owned());
                }
                validate_records(&file.records)?;
                file.records
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err("the isolated workspace registry cannot be read".to_owned()),
        };
        Ok(Self { path, records })
    }

    pub(crate) fn list(&self) -> Response {
        Response::IsolatedWorkspaces(
            self.records
                .iter()
                .filter(|record| record.state != State::Released)
                .map(Record::line)
                .collect(),
        )
    }

    pub(crate) async fn prepare(
        &mut self,
        containment: &Containment,
        request_id: &str,
        project: &str,
    ) -> Result<Response, String> {
        validate_uuid(request_id, "isolation request")?;
        if let Some(index) = self
            .records
            .iter()
            .position(|record| record.request_id.as_ref() == request_id)
        {
            let requested = AbsPath::canonicalize(project)
                .map_err(|_| "the selected project directory cannot be resolved".to_owned())?;
            let requested = ProjectIdentity::discover(requested)
                .map_err(|_| "the selected project identity cannot be resolved".to_owned())?;
            let existing = self
                .records
                .get(index)
                .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
            if &existing.project != requested.worktree() {
                return Err("the isolation request was already bound to another project".to_owned());
            }
            return self.finish_creation(index, containment).await;
        }

        self.make_room()?;
        let requested = AbsPath::canonicalize(project)
            .map_err(|_| "the selected project directory cannot be resolved".to_owned())?;
        let identity = ProjectIdentity::discover(requested)
            .map_err(|_| "the selected project identity cannot be resolved".to_owned())?;
        let base = identity.worktree().clone();
        let git = resolve("git").map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
        require_clean(&git, &base, containment).await?;
        let base_commit = revision(&git, &base, "HEAD", containment).await?;
        let parent = base
            .parent()
            .ok_or_else(|| "the selected project has no parent directory".to_owned())?;
        let workspace = parent
            .join(".runtrol-worktrees")
            .and_then(|root| root.join(&format!("chat-{request_id}")))
            .map_err(|_| "the isolated workspace path cannot be formed".to_owned())?;
        if workspace.as_std_path().exists() {
            return Err(
                "the isolated workspace target already exists without Core ownership".to_owned(),
            );
        }
        let record = Record {
            workspace_id: request_id.into(),
            request_id: request_id.into(),
            project: base,
            workspace,
            base_commit,
            session_id: None,
            state: State::Creating,
        };
        self.records.push(record);
        self.save()?;
        let index = self.records.len() - 1;
        self.finish_creation(index, containment).await
    }

    async fn finish_creation(
        &mut self,
        index: usize,
        containment: &Containment,
    ) -> Result<Response, String> {
        let record = self
            .records
            .get(index)
            .cloned()
            .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
        match record.state {
            State::Released => {
                return Err("the isolation request was already released".to_owned());
            }
            State::Bound | State::PreservedDirty => {
                verify_owned(&record)?;
                return Ok(Response::IsolatedWorkspace(Box::new(record.line())));
            }
            State::Ready => {
                verify_owned(&record)?;
                let git = resolve("git")
                    .map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
                if revision(&git, &record.workspace, "HEAD", containment).await?
                    != record.base_commit
                {
                    return Err(
                        "the unused isolated workspace moved from its frozen base".to_owned()
                    );
                }
                return Ok(Response::IsolatedWorkspace(Box::new(record.line())));
            }
            State::Creating => {}
        }

        if !record.workspace.as_std_path().exists() {
            let parent = record
                .workspace
                .parent()
                .ok_or_else(|| "the isolated workspace has no parent directory".to_owned())?;
            std::fs::create_dir_all(parent.as_std_path())
                .map_err(|_| "the isolated workspace parent cannot be created".to_owned())?;
            let git =
                resolve("git").map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
            let output = capture(
                &git,
                &[
                    "-C".to_owned(),
                    record.project.as_str().to_owned(),
                    "worktree".to_owned(),
                    "add".to_owned(),
                    "--detach".to_owned(),
                    record.workspace.as_str().to_owned(),
                    record.base_commit.to_string(),
                ],
                GIT_WORKTREE_TIMEOUT,
                containment,
            )
            .await
            .map_err(|_| "Git could not create the isolated workspace".to_owned())?;
            if !output.succeeded() || output.truncated {
                return Err("Git refused to create the isolated workspace".to_owned());
            }
        }
        verify_owned(&record)?;
        let git = resolve("git").map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
        if revision(&git, &record.workspace, "HEAD", containment).await? != record.base_commit {
            return Err("the created isolated workspace is not at its frozen base".to_owned());
        }
        let line = {
            let current = self
                .records
                .get_mut(index)
                .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
            current.state = State::Ready;
            current.line()
        };
        self.save()?;
        Ok(Response::IsolatedWorkspace(Box::new(line)))
    }

    pub(crate) fn bind(
        &mut self,
        workspace_id: &str,
        session_id: &str,
        workspace: &str,
    ) -> Result<Response, String> {
        validate_uuid(workspace_id, "isolated workspace")?;
        session_id
            .parse::<SessionId>()
            .map_err(|_| "the Runtime session identity is invalid".to_owned())?;
        let index = self
            .records
            .iter()
            .position(|record| record.workspace_id.as_ref() == workspace_id)
            .ok_or_else(|| "the isolated workspace is not owned by this Core".to_owned())?;
        let record = self
            .records
            .get_mut(index)
            .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
        if record.workspace.as_str() != workspace {
            return Err("the Runtime session opened in a different workspace".to_owned());
        }
        match record.state {
            State::Ready => {
                record.session_id = Some(session_id.into());
                record.state = State::Bound;
                self.save()?;
                Ok(Response::Done)
            }
            State::Bound if record.session_id.as_deref() == Some(session_id) => Ok(Response::Done),
            State::Bound => {
                Err("the isolated workspace is already bound to another session".to_owned())
            }
            State::Creating => Err("the isolated workspace is not ready".to_owned()),
            State::PreservedDirty => {
                Err("the isolated workspace is retained for dirty changes".to_owned())
            }
            State::Released => Err("the isolated workspace was already released".to_owned()),
        }
    }

    pub(crate) async fn release(
        &mut self,
        containment: &Containment,
        workspace_id: Option<&str>,
        session_id: Option<&str>,
        workspace: &str,
    ) -> Result<Response, String> {
        if workspace_id.is_none() && session_id.is_none() {
            return Err(
                "isolated workspace cleanup needs an ownership or session identity".to_owned(),
            );
        }
        if let Some(id) = workspace_id {
            validate_uuid(id, "isolated workspace")?;
        }
        if let Some(id) = session_id {
            id.parse::<SessionId>()
                .map_err(|_| "the Runtime session identity is invalid".to_owned())?;
        }
        let Some(index) = self.records.iter().position(|record| {
            record.workspace.as_str() == workspace
                && workspace_id.is_none_or(|id| record.workspace_id.as_ref() == id)
                && session_id.is_none_or(|id| {
                    record.session_id.as_deref().map_or_else(
                        || matches!(record.state, State::Ready | State::Creating),
                        |bound| bound == id,
                    )
                })
        }) else {
            return Ok(Response::Done);
        };
        let record = self
            .records
            .get(index)
            .cloned()
            .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
        if record.state == State::Released {
            return Ok(release_line(&record, "alreadyRemoved"));
        }
        if record.state == State::Creating && !record.workspace.as_std_path().exists() {
            let released = self.transition(index, State::Released)?;
            return Ok(release_line(&released, "removed"));
        }
        verify_owned(&record)?;
        let git = resolve("git").map_err(|_| "Git is unavailable for chat cleanup".to_owned())?;
        let head = revision(&git, &record.workspace, "HEAD", containment).await?;
        if head != record.base_commit || !is_clean(&git, &record.workspace, containment).await? {
            let preserved = self.transition(index, State::PreservedDirty)?;
            return Ok(release_line(&preserved, "preservedDirty"));
        }
        let output = capture(
            &git,
            &[
                "-C".to_owned(),
                record.project.as_str().to_owned(),
                "worktree".to_owned(),
                "remove".to_owned(),
                record.workspace.as_str().to_owned(),
            ],
            GIT_WORKTREE_TIMEOUT,
            containment,
        )
        .await
        .map_err(|_| "Git could not remove the isolated workspace".to_owned())?;
        if !output.succeeded() || output.truncated {
            return Err("Git refused to remove the clean isolated workspace".to_owned());
        }
        let released = self.transition(index, State::Released)?;
        Ok(release_line(&released, "removed"))
    }

    fn transition(&mut self, index: usize, state: State) -> Result<Record, String> {
        let current = self
            .records
            .get_mut(index)
            .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
        current.state = state;
        let current = current.clone();
        self.save()?;
        Ok(current)
    }

    fn make_room(&mut self) -> Result<(), String> {
        if self.records.len() < MAX_RECORDS {
            return Ok(());
        }
        if let Some(index) = self
            .records
            .iter()
            .position(|record| record.state == State::Released)
        {
            self.records.remove(index);
            return Ok(());
        }
        Err("the bounded isolated workspace registry is full".to_owned())
    }

    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&File {
            schema: FILE_SCHEMA,
            records: self.records.clone(),
        })
        .map_err(|_| "the isolated workspace registry cannot be encoded".to_owned())?;
        if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_FILE_BYTES) {
            return Err("the isolated workspace registry exceeds its fixed bound".to_owned());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "the isolated workspace registry has no parent".to_owned())?;
        let temporary = parent
            .join("isolated-workspaces.json.writing")
            .map_err(|_| "the isolated workspace temporary path is invalid".to_owned())?;
        let mut file = std::fs::File::create(temporary.as_std_path())
            .map_err(|_| "the isolated workspace registry cannot be created".to_owned())?;
        file.write_all(&bytes)
            .map_err(|_| "the isolated workspace registry cannot be written".to_owned())?;
        file.sync_all()
            .map_err(|_| "the isolated workspace registry cannot be flushed".to_owned())?;
        drop(file);
        std::fs::rename(temporary.as_std_path(), self.path.as_std_path())
            .map_err(|_| "the isolated workspace registry cannot be replaced".to_owned())
    }
}

fn validate_records(records: &[Record]) -> Result<(), String> {
    if records.len() > MAX_RECORDS {
        return Err("the isolated workspace registry exceeds its record bound".to_owned());
    }
    let mut requests = BTreeSet::new();
    let mut workspaces = BTreeSet::new();
    let mut sessions = BTreeSet::new();
    for record in records {
        validate_uuid(&record.request_id, "isolation request")?;
        validate_uuid(&record.workspace_id, "isolated workspace")?;
        if record.request_id != record.workspace_id {
            return Err("an isolated workspace identity changed from its request".to_owned());
        }
        if !requests.insert(record.request_id.as_ref())
            || !workspaces.insert(record.workspace.as_str())
        {
            return Err("the isolated workspace registry contains duplicate ownership".to_owned());
        }
        if let Some(session) = record.session_id.as_deref() {
            session
                .parse::<SessionId>()
                .map_err(|_| "the isolated workspace registry has an invalid session".to_owned())?;
            if !sessions.insert(session) {
                return Err("one Runtime session owns multiple isolated workspaces".to_owned());
            }
        }
        if record.state == State::Bound && record.session_id.is_none() {
            return Err("a bound isolated workspace has no Runtime session".to_owned());
        }
        if matches!(record.state, State::Creating | State::Ready) && record.session_id.is_some() {
            return Err(
                "an unbound isolated workspace unexpectedly names a Runtime session".to_owned(),
            );
        }
        if !valid_commit(&record.base_commit) {
            return Err("the isolated workspace registry has an invalid base commit".to_owned());
        }
        let parent = record
            .project
            .parent()
            .ok_or_else(|| "an isolated workspace target cannot be reconstructed".to_owned())?;
        let root = parent
            .join(".runtrol-worktrees")
            .map_err(|_| "an isolated workspace target cannot be reconstructed".to_owned())?;
        let expected = root
            .join(&format!("chat-{}", record.workspace_id))
            .map_err(|_| "an isolated workspace target cannot be reconstructed".to_owned())?;
        if expected != record.workspace {
            return Err("an isolated workspace target is outside the owned root".to_owned());
        }
    }
    Ok(())
}

fn validate_uuid(value: &str, kind: &str) -> Result<(), String> {
    let segments = [8, 4, 4, 4, 12];
    let mut parts = value.split('-');
    let valid = segments.into_iter().all(|length| {
        parts.next().is_some_and(|part| {
            part.len() == length
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }) && parts.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(format!("the {kind} identity is not a canonical UUID"))
    }
}

fn verify_owned(record: &Record) -> Result<(), String> {
    let workspace = AbsPath::canonicalize(record.workspace.as_str())
        .map_err(|_| "the owned isolated workspace is unavailable".to_owned())?;
    if workspace != record.workspace {
        return Err("the owned isolated workspace changed filesystem identity".to_owned());
    }
    let identity = ProjectIdentity::discover(workspace)
        .map_err(|_| "the owned isolated workspace identity cannot be resolved".to_owned())?;
    let base_identity = ProjectIdentity::discover(record.project.clone())
        .map_err(|_| "the isolated workspace base identity cannot be resolved".to_owned())?;
    if identity.worktree() == base_identity.worktree()
        || identity.common_store() != base_identity.common_store()
    {
        return Err("the owned isolated workspace no longer belongs to its project".to_owned());
    }
    Ok(())
}

async fn require_clean(
    git: &runtrol_childproc::Program,
    workspace: &AbsPath,
    containment: &Containment,
) -> Result<(), String> {
    if is_clean(git, workspace, containment).await? {
        Ok(())
    } else {
        Err("safe isolation requires a clean project checkout".to_owned())
    }
}

async fn is_clean(
    git: &runtrol_childproc::Program,
    workspace: &AbsPath,
    containment: &Containment,
) -> Result<bool, String> {
    let output = capture(
        git,
        &[
            "-C".to_owned(),
            workspace.as_str().to_owned(),
            "status".to_owned(),
            "--porcelain=v1".to_owned(),
            "--untracked-files=all".to_owned(),
        ],
        GIT_INSPECTION_TIMEOUT,
        containment,
    )
    .await
    .map_err(|_| "Git could not inspect the workspace".to_owned())?;
    if !output.succeeded() || output.truncated {
        return Err("Git could not prove the workspace state".to_owned());
    }
    Ok(output.stdout.is_empty())
}

async fn revision(
    git: &runtrol_childproc::Program,
    workspace: &AbsPath,
    name: &str,
    containment: &Containment,
) -> Result<Box<str>, String> {
    let output = capture(
        git,
        &[
            "-C".to_owned(),
            workspace.as_str().to_owned(),
            "rev-parse".to_owned(),
            "--verify".to_owned(),
            "--end-of-options".to_owned(),
            format!("{name}^{{commit}}"),
        ],
        GIT_INSPECTION_TIMEOUT,
        containment,
    )
    .await
    .map_err(|_| "Git could not resolve the project base".to_owned())?;
    if !output.succeeded() || output.truncated {
        return Err("the project has no resolvable HEAD commit".to_owned());
    }
    let value = core::str::from_utf8(&output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 base commit".to_owned())?
        .trim()
        .to_ascii_lowercase();
    if !valid_commit(&value) {
        return Err("Git returned an invalid base commit".to_owned());
    }
    Ok(value.into())
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn release_line(record: &Record, outcome: &str) -> Response {
    Response::IsolatedWorkspaceReleased(Box::new(IsolatedWorkspaceReleaseLine {
        workspace_id: record.workspace_id.clone(),
        workspace: record.workspace.as_str().into(),
        outcome: outcome.into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        root: std::path::PathBuf,
        project: AbsPath,
        registry: AbsPath,
    }

    impl Scratch {
        fn make() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-isolated-workspace-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("project")).expect("create project");
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
            Self {
                project: AbsPath::canonicalize(root.join("project").to_string_lossy().as_ref())
                    .expect("canonical project"),
                registry: AbsPath::canonicalize(root.join("home").to_string_lossy().as_ref())
                    .expect("canonical home")
                    .join("isolated-workspaces.json")
                    .expect("registry path"),
                root,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if self.root.exists() {
                std::fs::remove_dir_all(&self.root).expect("remove scratch tree");
            }
        }
    }

    fn git(project: &std::path::Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(arguments)
            .status()
            .expect("run Git fixture command");
        assert!(
            status.success(),
            "Git fixture command failed: {arguments:?}"
        );
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
    async fn cleanup_preserves_changes_across_restart_and_refuses_a_dirty_base() {
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

        let mut after_restart = IsolatedWorkspaceController::open(scratch.registry.clone())
            .expect("restore dirty record");
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

        std::fs::write(
            scratch.project.as_std_path().join("dirty-base.txt"),
            b"dirty\n",
        )
        .expect("dirty base");
        let refusal = after_restart
            .prepare(
                &containment,
                "21234567-89ab-cdef-0123-456789abcdef",
                scratch.project.as_str(),
            )
            .await
            .expect_err("dirty base refused");
        assert!(refusal.contains("requires a clean project checkout"));
    }
}
