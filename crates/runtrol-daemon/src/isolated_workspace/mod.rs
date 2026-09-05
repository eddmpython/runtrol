//! Core-owned linked worktrees for ordinary parallel chats.
//!
//! Studio asks for isolation but never creates, guesses, or deletes a worktree. This controller freezes the
//! selected checkout's committed `HEAD`, records ownership before asking Git to create anything, binds the result to
//! the public Runtime session, and removes only an exact owned worktree that Git proves is clean. Conversation
//! bytes, provider identities, branches, commits, and integration policy are deliberately absent.

use std::collections::BTreeSet;
use std::time::Duration;

use runtrol_childproc::{Containment, resolve};
use runtrol_core::project::ProjectIdentity;
use runtrol_ipc::wire::{IsolatedWorkspaceLine, IsolatedWorkspaceReleaseLine, Response};
use runtrol_provider::{AbsPath, SessionId};
use serde::{Deserialize, Serialize};

mod identity;
pub(crate) mod ownership;
mod recovery;
mod registry;
mod terminal;
pub(crate) use identity::VerifiedProject;
pub(crate) use recovery::{recover_after_restart, report_cleanup};
pub(crate) use terminal::PreparedWorkspace;

const FILE_SCHEMA: u8 = 2;
const MAX_RECORDS: usize = 128;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const GIT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(15);
const GIT_WORKTREE_TIMEOUT: Duration = Duration::from_mins(1);

struct Operation<'a> {
    containment: &'a Containment,
    lease: std::sync::Arc<std::fs::File>,
}

async fn capture(
    git: &runtrol_childproc::Program,
    args: &[String],
    within: Duration,
    operation: &Operation<'_>,
) -> Result<runtrol_childproc::Output, runtrol_childproc::SpawnError> {
    #[cfg(windows)]
    {
        runtrol_childproc::capture_retaining(
            git,
            args,
            within,
            operation.containment,
            operation.lease.clone(),
        )
        .await
    }
    #[cfg(not(windows))]
    {
        let _held = &operation.lease;
        runtrol_childproc::capture(git, args, within, operation.containment).await
    }
}

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
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    terminal: Option<terminal::TerminalRecord>,
    #[serde(default)]
    legacy: bool,
}

impl Record {
    fn require_current_owner(&self) -> Result<(), String> {
        if self.legacy {
            return Err("legacy worktree ownership is preserved because its operation and process identities were not recorded".to_owned());
        }
        Ok(())
    }
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
        let records = registry::read(&path)?;
        Ok(Self { path, records })
    }

    fn refresh_for_write(&mut self) -> Result<(), String> {
        registry::check_writable(&self.path)?;
        self.records = registry::read(&self.path)?;
        Ok(())
    }

    pub(crate) fn list(&self) -> Response {
        let records = match registry::read(&self.path) {
            Ok(records) => records,
            Err(message) => return Response::Failed(runtrol_ipc::wire::WireError::plain(&message)),
        };
        Response::IsolatedWorkspaces(
            records
                .iter()
                .filter(|record| record.state != State::Released && record.terminal.is_none())
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
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, request_id)?,
        };
        self.refresh_for_write()?;
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
            if existing.terminal.is_some() {
                return Err("terminal-owned worktrees require their exact spawn ticket".to_owned());
            }
            if &existing.project != requested.worktree() {
                return Err("the isolation request was already bound to another project".to_owned());
            }
            return self.finish_creation(index, &operation).await;
        }

        self.make_room()?;
        let requested = AbsPath::canonicalize(project)
            .map_err(|_| "the selected project directory cannot be resolved".to_owned())?;
        let identity = ProjectIdentity::discover(requested)
            .map_err(|_| "the selected project identity cannot be resolved".to_owned())?;
        let base = identity.worktree().clone();
        let git = resolve("git").map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
        let base_commit = revision(&git, &base, "HEAD", &operation).await?;
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
            revision: 0,
            terminal: None,
            legacy: false,
        };
        self.records.push(record);
        self.save(request_id)?;
        let index = self.records.len() - 1;
        self.finish_creation(index, &operation).await
    }

    async fn finish_creation(
        &mut self,
        index: usize,
        operation: &Operation<'_>,
    ) -> Result<Response, String> {
        let record = self
            .records
            .get(index)
            .cloned()
            .ok_or_else(|| "the isolated workspace ownership record disappeared".to_owned())?;
        record.require_current_owner()?;
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
                if revision(&git, &record.workspace, "HEAD", operation).await? != record.base_commit
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
                operation,
            )
            .await
            .map_err(|_| "Git could not create the isolated workspace".to_owned())?;
            if !output.succeeded() || output.truncated {
                return Err("Git refused to create the isolated workspace".to_owned());
            }
        }
        verify_owned(&record)?;
        let git = resolve("git").map_err(|_| "Git is unavailable for chat isolation".to_owned())?;
        if revision(&git, &record.workspace, "HEAD", operation).await? != record.base_commit {
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
        self.save(&record.workspace_id)?;
        Ok(Response::IsolatedWorkspace(Box::new(line)))
    }

    pub(crate) fn bind(
        &mut self,
        workspace_id: &str,
        session_id: &str,
        workspace: &str,
    ) -> Result<Response, String> {
        validate_uuid(workspace_id, "isolated workspace")?;
        let _operation = registry::operation(&self.path, workspace_id)?;
        self.refresh_for_write()?;
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
        record.require_current_owner()?;
        if record.terminal.is_some() {
            return Err("terminal-owned worktrees cannot bind to a structured session".to_owned());
        }
        if record.workspace.as_str() != workspace {
            return Err("the Runtime session opened in a different workspace".to_owned());
        }
        match record.state {
            State::Ready => {
                record.session_id = Some(session_id.into());
                record.state = State::Bound;
                self.save(workspace_id)?;
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
        self.refresh_for_write()?;
        let Some(candidate) = self
            .records
            .iter()
            .find(|record| record.workspace.as_str() == workspace)
        else {
            return Ok(Response::Done);
        };
        let operation = Operation {
            containment,
            lease: registry::operation(&self.path, &candidate.workspace_id)?,
        };
        self.refresh_for_write()?;
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
        if record.terminal.is_some() {
            return Err("terminal-owned worktrees require an ended spawn permit".to_owned());
        }
        record.require_current_owner()?;
        if record.state == State::Released {
            return Ok(release_line(&record, "alreadyRemoved"));
        }
        if record.state == State::Creating && !record.workspace.as_std_path().exists() {
            let released = self.transition(index, State::Released)?;
            return Ok(release_line(&released, "removed"));
        }
        verify_owned(&record)?;
        let git = resolve("git").map_err(|_| "Git is unavailable for chat cleanup".to_owned())?;
        let head = revision(&git, &record.workspace, "HEAD", &operation).await?;
        if head != record.base_commit || !is_clean(&git, &record.workspace, &operation).await? {
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
            &operation,
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
        self.save(&current.workspace_id)?;
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

    fn save(&mut self, workspace_id: &str) -> Result<(), String> {
        let changed = self
            .records
            .iter()
            .find(|record| record.workspace_id.as_ref() == workspace_id)
            .cloned()
            .ok_or("the worktree ownership disappeared")?;
        self.records = registry::update(&self.path, changed)?;
        Ok(())
    }
}

fn validate_records(records: &[Record]) -> Result<(), String> {
    if records.len() > MAX_RECORDS {
        return Err("the isolated workspace registry exceeds its record bound".to_owned());
    }
    let mut requests = BTreeSet::new();
    let mut workspaces = BTreeSet::new();
    let mut sessions = BTreeSet::new();
    let mut terminals = BTreeSet::new();
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
        if record.state == State::Bound && record.session_id.is_none() && record.terminal.is_none()
        {
            return Err("a bound isolated workspace has no Runtime session".to_owned());
        }
        if matches!(record.state, State::Creating | State::Ready) && record.session_id.is_some() {
            return Err(
                "an unbound isolated workspace unexpectedly names a Runtime session".to_owned(),
            );
        }
        if let Some(terminal) = &record.terminal {
            terminal.validate(record)?;
            if !terminals.insert(terminal.ticket.worker) {
                return Err("one Runtime terminal owns multiple isolated workspaces".to_owned());
            }
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
    if let Some(terminal) = &record.terminal {
        return terminal.verify(record, true);
    }
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

async fn is_clean(
    git: &runtrol_childproc::Program,
    workspace: &AbsPath,
    operation: &Operation<'_>,
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
        operation,
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
    operation: &Operation<'_>,
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
        operation,
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
mod tests;
