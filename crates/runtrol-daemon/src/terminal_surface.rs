//! Shared hosted provider terminal ownership behind the public Runtime surface.
//!
//! What this is (`docs/terminalSurface.md`): the conversation surface is the provider's own terminal
//! interface, run by the daemon on a pseudo terminal it owns, shown by any number of viewers at once. The
//! daemon side of that is [`runtrol_core::terminal::Terminal`]. It reads no byte for conversation meaning.
//!
//! The launch is the manifest's word (`[tui]`): the program the probe resolved, the manifest's `new` or
//! `resume` arguments, its `env` and `env_unset`. Nothing here knows a provider by name.

use std::collections::BTreeMap;
use std::sync::Arc;

use runtrol_core::terminal::{Attachment, Terminal, TerminalLaunch};
use runtrol_provider::{AbsPath, ProviderId, TerminalId, WallMs};

use crate::compose::Composed;
use crate::native_claims::{TerminalClaimAdmission, TerminalClaimError};

const MAX_BROKER_ARGUMENTS: usize = 256;
const MAX_BROKER_ARGUMENT_BYTES: usize = 256 * 1024;
/// Hosted terminals have the same bounded process count as the structured hot-session engine.
pub(crate) const MAX_HOSTED_TERMINALS: usize = runtrol_core::session::MAX_HOT;

#[derive(Debug, thiserror::Error)]
pub(crate) enum TerminalOpenError {
    #[error(transparent)]
    Claim(#[from] TerminalClaimError),
    #[error("the hosted terminal limit is {limit}, and all {held} slots are occupied")]
    NoRoom { held: usize, limit: usize },
    #[error("{0}")]
    Provider(String),
}

/// Every open terminal, by id and by the conversation it shows.
pub(crate) struct Terminals {
    by_id: BTreeMap<TerminalId, Open>,
    /// Which terminal shows which conversation, so a second open joins the first.
    by_conversation: BTreeMap<(ProviderId, Box<str>), TerminalId>,
    /// Structural table generation. Terminal content never enters this publisher.
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for Terminals {
    fn default() -> Self {
        let (changes, _initial) = tokio::sync::watch::channel(0);
        Self {
            by_id: BTreeMap::new(),
            by_conversation: BTreeMap::new(),
            changes,
        }
    }
}

/// One open terminal, whose service it belongs to, and the folder its CLI runs in.
struct Open {
    /// Which service is hosted here, so a question about that service's account can be asked when this
    /// terminal's CLI stops writing. Known even for a conversation the service has not named yet.
    provider: ProviderId,
    terminal: Terminal,
    /// The canonical folder, so a join is judged against the folder the conversation really runs in,
    /// never against the folder the joining request happened to name.
    workspace: AbsPath,
    /// Provider-owned identity known before launch, never inferred from output.
    native: Option<Box<str>>,
    /// Structural process start time.
    opened_at_ms: u64,
    /// Process incarnation for stale lease checks. Terminal IDs are unique, so the first incarnation is one.
    generation: u64,
    /// A public stop was accepted and process exit is pending.
    stopping: bool,
}

/// A content-free view of one hosted terminal for public Runtime projection.
#[derive(Clone)]
pub(crate) struct HostedTerminal {
    pub(crate) id: TerminalId,
    pub(crate) provider: ProviderId,
    pub(crate) terminal: Terminal,
    pub(crate) workspace: AbsPath,
    pub(crate) native: Option<Box<str>>,
    pub(crate) opened_at_ms: u64,
    pub(crate) generation: u64,
    pub(crate) stopping: bool,
}

impl HostedTerminal {
    fn from_open(id: TerminalId, open: &Open) -> Self {
        Self {
            id,
            provider: open.provider,
            terminal: open.terminal.clone(),
            workspace: open.workspace.clone(),
            native: open.native.clone(),
            opened_at_ms: open.opened_at_ms,
            generation: open.generation,
            stopping: open.stopping,
        }
    }
}

impl Terminals {
    /// One content-free hosted terminal record.
    pub(crate) fn hosted(&self, id: TerminalId) -> Option<HostedTerminal> {
        self.by_id
            .get(&id)
            .map(|open| HostedTerminal::from_open(id, open))
    }

    /// Every live content-free terminal record in stable identity order.
    pub(crate) fn hosted_all(&self) -> Vec<HostedTerminal> {
        self.by_id
            .iter()
            .map(|(id, open)| HostedTerminal::from_open(*id, open))
            .collect()
    }

    /// Structural changes used by the public root-filtered terminal index.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The terminal already showing this known provider-native conversation.
    pub(crate) fn open_for(&self, provider: ProviderId, native: &str) -> Option<HostedTerminal> {
        let id = *self.by_conversation.get(&(provider, native.into()))?;
        let open = self.by_id.get(&id)?;
        Some(HostedTerminal::from_open(id, open))
    }

    /// Bind a provider-minted identity to the exact PTY process that its roster names.
    pub(crate) fn bind_native_process(
        &mut self,
        claims: &crate::native_claims::NativeLiveClaimRegistry,
        provider: ProviderId,
        pid: u32,
        native: &str,
    ) -> Result<bool, TerminalClaimError> {
        let Some((terminal_id, open)) = self
            .by_id
            .iter_mut()
            .find(|(_, open)| open.provider == provider && open.terminal.pid() == pid)
        else {
            return Ok(false);
        };
        if open.native.as_deref() == Some(native) {
            return Ok(false);
        }
        if open.native.is_some() {
            return Err(TerminalClaimError::State);
        }
        claims.bind_terminal_native(
            *terminal_id,
            provider.as_str(),
            native,
            open.workspace.as_str(),
        )?;
        open.native = Some(native.into());
        let terminal_id = *terminal_id;
        self.by_conversation
            .insert((provider, native.into()), terminal_id);
        self.publish_change();
        Ok(true)
    }

    fn insert(
        &mut self,
        id: TerminalId,
        provider: ProviderId,
        key: Option<(ProviderId, Box<str>)>,
        terminal: Terminal,
        workspace: AbsPath,
        native: Option<Box<str>>,
    ) {
        self.by_id.insert(
            id,
            Open {
                provider,
                terminal,
                workspace,
                native,
                opened_at_ms: WallMs::now().as_millis(),
                generation: 1,
                stopping: false,
            },
        );
        if let Some(key) = key {
            self.by_conversation.insert(key, id);
        }
        self.publish_change();
    }

    fn remove(&mut self, id: TerminalId) -> Option<HostedTerminal> {
        let removed = self
            .by_id
            .remove(&id)
            .map(|open| HostedTerminal::from_open(id, &open));
        self.by_conversation.retain(|_, open| *open != id);
        if removed.is_some() {
            self.publish_change();
        }
        removed
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Mark a terminal as stopping before the asynchronous process exit arrives.
    pub(crate) fn mark_stopping(&mut self, id: TerminalId) -> bool {
        let Some(open) = self.by_id.get_mut(&id) else {
            return false;
        };
        if !open.stopping {
            open.stopping = true;
            self.publish_change();
        }
        true
    }

    /// Publish a geometry-only descriptor change after the PTY accepted a resize.
    pub(crate) fn publish_geometry_change(&self) {
        self.publish_change();
    }

    fn publish_change(&self) {
        let next = self.changes.borrow().wrapping_add(1);
        self.changes.send_replace(next);
    }

    /// For every service with a terminal open, when its CLI last wrote anything.
    ///
    /// The one signal a conversation held as a terminal has for "something happened". A service with a
    /// terminal that has never written is present with no instant, which is what a terminal opened a
    /// moment ago looks like.
    pub(crate) fn wrote_at_by_provider(&self) -> BTreeMap<ProviderId, Option<WallMs>> {
        let mut latest: BTreeMap<ProviderId, Option<WallMs>> = BTreeMap::new();
        for open in self.by_id.values() {
            let entry = latest.entry(open.provider).or_default();
            let wrote = open.terminal.wrote_at();
            if wrote > *entry {
                *entry = wrote;
            }
        }
        latest
    }
}

/// Drop a terminal from the table and tell the owner, which may be waiting on it to drain.
async fn forget(composed: &Composed, id: TerminalId) {
    let open = {
        let mut terminals = composed.terminals.lock().await;
        drop(terminals.remove(id));
        terminals.len()
    };
    composed
        .open_terminals
        .store(open, std::sync::atomic::Ordering::Release);
    composed.runtime_terminals.terminal_ended(id).await;
    composed.native_claims.terminal_ended(id);
    composed.terminal_closed.notify_one();
}

/// Drop the terminal from the table the moment its CLI ends, viewer or no viewer. Without this a
/// conversation whose viewer left first stayed in the table forever, kept the draining rule from ever
/// reaching zero, and answered the next open with an already-ended terminal (measured 2026-08-25).
fn forget_on_exit(composed: Arc<Composed>, id: TerminalId, terminal: &Terminal) {
    let mut exited = terminal.exited();
    tokio::spawn(async move {
        loop {
            if exited.borrow().is_some() {
                break;
            }
            if exited.changed().await.is_err() {
                break;
            }
        }
        forget(&composed, id).await;
    });
}

/// Open or join the one shared terminal table after the caller validated provider and root authority.
pub(crate) async fn open_hosted(
    composed: &Arc<Composed>,
    id: ProviderId,
    native: Option<&str>,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
    prepared_program: Option<runtrol_childproc::Program>,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    open_with_arguments(
        composed,
        id,
        native,
        cwd,
        cols,
        rows,
        prepared_program,
        None,
    )
    .await
}

/// Open an exact local provider invocation and make the invoking terminal its first viewer.
///
/// Provider identity and executable still come from the runtime registry. Arguments are the local operator's exact
/// argv and are never interpreted for meaning. A native identity is bound only when they structurally match the
/// manifest's discovered resume prefix followed by one opaque id.
pub(crate) async fn open_brokered(
    composed: &Arc<Composed>,
    id: ProviderId,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
    arguments: Vec<String>,
    prepared_program: runtrol_childproc::Program,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    if arguments.len() > MAX_BROKER_ARGUMENTS
        || arguments.iter().map(String::len).sum::<usize>() > MAX_BROKER_ARGUMENT_BYTES
    {
        return Err(TerminalOpenError::Provider(
            "the local provider invocation exceeds the bounded argument budget".to_owned(),
        ));
    }
    let declared = composed
        .registry
        .get(id)
        .ok_or_else(|| TerminalOpenError::Provider(format!("no provider called {id}")))?;
    let tui = declared.manifest.tui.as_ref().ok_or_else(|| {
        TerminalOpenError::Provider(format!("{id} declares no terminal interface"))
    })?;
    let native = declared_resume_native(tui, &arguments);
    open_with_arguments(
        composed,
        id,
        native.as_deref(),
        cwd,
        cols,
        rows,
        Some(prepared_program),
        Some(arguments),
    )
    .await
}

/// Native identity carried by the one exact resume spelling the provider manifest declares.
///
/// Arbitrary local argv remains opaque. Recognizing only the complete declared prefix plus one opaque final word
/// lets a second invocation join the original PTY without teaching the broker a provider flag or guessing at an
/// alias the provider did not publish here.
fn declared_resume_native(tui: &runtrol_provider::TuiSpec, arguments: &[String]) -> Option<String> {
    (!tui.resume.is_empty()
        && arguments.len() == tui.resume.len().saturating_add(1)
        && arguments
            .iter()
            .zip(tui.resume.iter())
            .all(|(argument, expected)| argument == expected.as_ref()))
    .then(|| arguments.last().cloned())
    .flatten()
}

#[expect(
    clippy::too_many_arguments,
    reason = "one launch boundary keeps provider, native claim, workspace, geometry, prepared executable, and exact argv visible"
)]
async fn open_with_arguments(
    composed: &Arc<Composed>,
    id: ProviderId,
    native: Option<&str>,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
    prepared_program: Option<runtrol_childproc::Program>,
    exact_arguments: Option<Vec<String>>,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    if let Some(native) = native
        && let Some(existing) = composed.terminals.lock().await.open_for(id, native)
    {
        if existing.workspace != cwd {
            return Err(TerminalClaimError::WorkspaceConflict.into());
        }
        let attachment = existing.terminal.attach().await;
        return Ok((existing.id, existing.terminal, attachment));
    }
    let terminal_id = TerminalId::now();
    let reservation = match composed.native_claims.reserve_terminal(
        terminal_id,
        id.as_str(),
        native,
        cwd.as_str(),
    )? {
        TerminalClaimAdmission::Join(existing) => {
            let (hosted, attachment) = attach_current(composed, existing)
                .await
                .map_err(TerminalOpenError::Provider)?;
            return Ok((hosted.id, hosted.terminal, attachment));
        }
        TerminalClaimAdmission::Reserved(reservation) => reservation,
    };
    let declared = composed
        .registry
        .get(id)
        .ok_or_else(|| TerminalOpenError::Provider(format!("no provider called {id}")))?;
    let tui = declared.manifest.tui.as_ref().ok_or_else(|| {
        TerminalOpenError::Provider(format!("{id} declares no terminal interface"))
    })?;
    let arguments: Vec<String> = match exact_arguments {
        Some(arguments) => arguments,
        None => match native {
            Some(native) => {
                if tui.resume.is_empty() {
                    return Err(TerminalOpenError::Provider(format!(
                        "{id} publishes no way to reopen a conversation from the command line"
                    )));
                }
                tui.resume
                    .iter()
                    .map(ToString::to_string)
                    .chain(std::iter::once(native.to_owned()))
                    .collect()
            }
            None => tui.new.iter().map(ToString::to_string).collect(),
        },
    };
    let program = if let Some(program) = prepared_program {
        program
    } else {
        let mut cache = runtrol_core::ProbeCache::open(composed.home.paths().probe_cache());
        let (program, _probed) =
            runtrol_core::probe_program(&declared.manifest, &[], &mut cache, &composed.containment)
                .await
                .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        program
    };
    let key = native.map(|native| (id, Box::<str>::from(native)));
    let native = native.map(Box::<str>::from);
    let (terminal, open) = {
        let mut terminals = composed.terminals.lock().await;
        if terminals.len() >= MAX_HOSTED_TERMINALS {
            return Err(TerminalOpenError::NoRoom {
                held: terminals.len(),
                limit: MAX_HOSTED_TERMINALS,
            });
        }
        let terminal = Terminal::open(&TerminalLaunch {
            program: &program,
            arguments,
            cwd: &cwd,
            env: tui
                .env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            env_unset: tui.env_unset.iter().map(ToString::to_string).collect(),
            size: runtrol_childproc::PtySize { cols, rows },
        })
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        terminals.insert(terminal_id, id, key, terminal.clone(), cwd, native);
        let open = terminals.len();
        (terminal, open)
    };
    composed
        .open_terminals
        .store(open, std::sync::atomic::Ordering::Release);
    if let Err(error) = reservation.commit() {
        drop(terminal.kill());
        forget(composed, terminal_id).await;
        return Err(error.into());
    }
    forget_on_exit(Arc::clone(composed), terminal_id, &terminal);
    let attachment = terminal.attach().await;
    Ok((terminal_id, terminal, attachment))
}

/// Attach without resizing. Public attach never grants geometry authority implicitly.
pub(crate) async fn attach_current(
    composed: &Composed,
    id: TerminalId,
) -> Result<(HostedTerminal, Attachment), String> {
    let hosted = composed
        .terminals
        .lock()
        .await
        .hosted(id)
        .ok_or_else(|| format!("no open terminal {id}"))?;
    let attachment = hosted.terminal.attach().await;
    Ok((hosted, attachment))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn only_the_manifest_declared_resume_shape_becomes_a_native_join_key() {
        let tui = runtrol_provider::TuiSpec {
            resume: vec!["--resume".into()],
            ..runtrol_provider::TuiSpec::default()
        };
        assert_eq!(
            declared_resume_native(&tui, &["--resume".to_owned(), "native-id".to_owned()]),
            Some("native-id".to_owned())
        );
        assert_eq!(
            declared_resume_native(&tui, &["-r".to_owned(), "native-id".to_owned()]),
            None
        );
        assert_eq!(declared_resume_native(&tui, &["--resume".to_owned()]), None);
    }

    /// Process birth reaches every terminal-index watcher through this publisher. The 50 ms hard ceiling is the
    /// daemon half of the sidebar discovery contract; provider preparation and catalogue reads are outside it.
    #[tokio::test]
    async fn terminal_registry_publication_p95_is_below_fifty_milliseconds() {
        const SAMPLES: usize = 512;
        const BUDGET: Duration = Duration::from_millis(50);

        let terminals = Terminals::default();
        let mut changes = terminals.changes();
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            terminals.publish_change();
            changes
                .changed()
                .await
                .expect("the registry publisher remains alive");
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let p95 = *samples
            .get(SAMPLES * 95 / 100)
            .expect("the fixed sample set has its p95 member");
        assert!(
            p95 <= BUDGET,
            "terminal registry publication p95 was {p95:?}, over {BUDGET:?}"
        );
    }

    #[test]
    fn hosted_terminal_count_has_the_same_hard_ceiling_as_hot_sessions() {
        assert_eq!(MAX_HOSTED_TERMINALS, runtrol_core::session::MAX_HOT);
        const {
            assert!(
                MAX_HOSTED_TERMINALS * runtrol_core::terminal::MAX_SHARED_TERMINAL_STATE_BYTES
                    <= 24 * 1024 * 1024,
                "the complete central terminal set exceeds its 24 MiB state budget"
            );
        }
    }
}
