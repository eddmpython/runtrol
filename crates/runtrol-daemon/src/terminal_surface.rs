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
use crate::window_registry::ConnectionToken;

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
    /// The observed shell already has the transparent shim's own terminal as its row.
    #[error("the transparent shim already brokers this shell's command")]
    AlreadyBrokered,
    /// The caller is not the connection feeding this observed mirror, or the terminal is no mirror.
    #[error("no observed mirror fed by this connection has that identity")]
    NotFedByCaller,
}

/// Every open terminal, by id and by the conversation it shows.
pub(crate) struct Terminals {
    by_id: BTreeMap<TerminalId, Open>,
    /// Which terminal shows which conversation, so a second open joins the first.
    by_conversation: BTreeMap<(ProviderId, Box<str>), TerminalId>,
    /// Which shell invoked which brokered terminal, so a window observing that shell does not mirror it twice.
    brokered_shells: BTreeMap<u32, TerminalId>,
    /// Structural table generation. Terminal content never enters this publisher.
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for Terminals {
    fn default() -> Self {
        let (changes, _initial) = tokio::sync::watch::channel(0);
        Self {
            by_id: BTreeMap::new(),
            by_conversation: BTreeMap::new(),
            brokered_shells: BTreeMap::new(),
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
    /// Provider-roster process structurally attributed to this owned PTY for the current native identity.
    /// Remembering it avoids a whole-system ancestry snapshot on every unchanged provider observation.
    native_process_pid: Option<u32>,
    /// Structural process start time.
    opened_at_ms: u64,
    /// Process incarnation for stale lease checks. Terminal IDs are unique, so the first incarnation is one.
    generation: u64,
    /// A public stop was accepted and process exit is pending.
    stopping: bool,
    /// Who owns the live conversation and how this terminal reaches it.
    origin: TerminalOrigin,
}

/// Who owns the live conversation behind one terminal renderer.
#[derive(Clone, Debug)]
pub(crate) enum TerminalOrigin {
    /// Runtime started and owns the provider TUI process on this PTY.
    Owned,
    /// Runtime joined an operating-system console owned elsewhere.
    ConsoleMirror,
    /// Runtime started only the provider's official TUI attachment client. The conversation owner remains
    /// elsewhere and is stopped through the paired provider command.
    OfficialAttach(Box<OfficialStop>),
    /// A registered VS Code window owns the terminal and feeds its raw execution output here.
    ObservedMirror(Box<ObservedOwner>),
}

impl TerminalOrigin {
    /// The public projection of this origin and, for an observed mirror, the window that owns it.
    pub(crate) fn projection(
        &self,
    ) -> (
        runtrol_runtime_protocol::TerminalOrigin,
        Option<&ObservedOwner>,
    ) {
        use runtrol_runtime_protocol::TerminalOrigin as Public;
        match self {
            Self::Owned => (Public::Owned, None),
            Self::ConsoleMirror => (Public::ConsoleMirror, None),
            Self::OfficialAttach(_) => (Public::OfficialAttach, None),
            Self::ObservedMirror(owner) => (Public::ObservedMirror, Some(owner)),
        }
    }
}

/// The window that owns an observed mirror, by the identities it registered.
#[derive(Clone, Debug)]
pub(crate) struct ObservedOwner {
    pub(crate) window_session_id: String,
    pub(crate) terminal_key: String,
    /// The connection that feeds the mirror; only it may feed or end it, and its end ends the mirror.
    pub(crate) feeder: ConnectionToken,
    /// The observed shell, when the window resolved one: the key a brokered open of the same shell retires.
    pub(crate) shell_pid: Option<u32>,
}

/// The provider's exact command for stopping a conversation reached through an official attachment.
#[derive(Clone, Debug)]
pub(crate) struct OfficialStop {
    program: runtrol_childproc::Program,
    arguments: Vec<String>,
    cwd: AbsPath,
    env: Vec<(String, String)>,
    env_unset: Vec<String>,
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
    /// Who owns the live conversation behind this renderer.
    pub(crate) origin: TerminalOrigin,
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
            origin: open.origin.clone(),
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

    /// Publisher used by a background resource sample to invalidate terminal descriptors.
    pub(crate) fn change_sender(&self) -> tokio::sync::watch::Sender<u64> {
        self.changes.clone()
    }

    /// The terminal already showing this known provider-native conversation.
    pub(crate) fn open_for(&self, provider: ProviderId, native: &str) -> Option<HostedTerminal> {
        let id = *self.by_conversation.get(&(provider, native.into()))?;
        let open = self.by_id.get(&id)?;
        Some(HostedTerminal::from_open(id, open))
    }

    /// Whether binding this provider's current process roster needs an operating-system ancestry query.
    ///
    /// Exact root-PID matches need no query. An already-bound descendant also needs no repeated process-table walk
    /// while the provider roster still maps that exact PID to that exact native identity. The ancestry snapshot is
    /// therefore paid only while a new or moved terminal identity is unresolved.
    pub(crate) fn needs_process_tree(
        &self,
        provider: ProviderId,
        activity: &runtrol_provider::NativeProcessActivity,
    ) -> bool {
        for open in self.by_id.values().filter(|open| {
            open.provider == provider && matches!(open.origin, TerminalOrigin::Owned)
        }) {
            let root = open.terminal.pid();
            if activity.processes.iter().any(|process| process.pid == root) {
                continue;
            }
            if open.native_process_pid.is_some_and(|pid| {
                activity.processes.iter().any(|process| {
                    process.pid == pid && open.native.as_deref() == Some(process.native.as_str())
                })
            }) {
                continue;
            }
            if !activity.processes.is_empty() {
                return true;
            }
        }
        false
    }

    /// Bind one complete provider process observation to exact PTY-owned process trees.
    ///
    /// One PID must belong to exactly one terminal tree, one terminal tree must name exactly one native identity, and
    /// one native identity must belong to exactly one terminal. Anything structurally ambiguous stays unresolved.
    /// Sibling terminals in one workspace are committed as one claim batch so neither unnamed sibling falsely blocks
    /// the other while both are being promoted.
    pub(crate) fn bind_native_processes<'a>(
        &mut self,
        claims: &crate::native_claims::NativeLiveClaimRegistry,
        process_tree: Option<&runtrol_childproc::ProcessTree>,
        provider: ProviderId,
        processes: impl IntoIterator<Item = (u32, &'a str)>,
    ) -> Vec<(u32, TerminalClaimError)> {
        let mut by_terminal = BTreeMap::<TerminalId, BTreeMap<&'a str, u32>>::new();
        for (pid, native) in processes {
            let matches = self
                .by_id
                .iter()
                .filter(|(_, open)| {
                    open.provider == provider
                        && matches!(open.origin, TerminalOrigin::Owned)
                        && (open.terminal.pid() == pid
                            || process_tree
                                .is_some_and(|tree| tree.contains(open.terminal.pid(), pid)))
                })
                .map(|(terminal_id, _)| *terminal_id)
                .collect::<Vec<_>>();
            if let [terminal_id] = matches.as_slice() {
                by_terminal
                    .entry(*terminal_id)
                    .or_default()
                    .entry(native)
                    .or_insert(pid);
            }
        }

        let candidates = by_terminal
            .into_iter()
            .filter_map(|(terminal_id, natives)| {
                let mut natives = natives.into_iter();
                let (native, pid) = natives.next()?;
                if natives.next().is_some() {
                    return None;
                }
                Some((terminal_id, pid, native))
            })
            .collect::<Vec<_>>();
        let mut native_owners = BTreeMap::<&str, Option<TerminalId>>::new();
        for (terminal_id, _pid, native) in &candidates {
            native_owners
                .entry(native)
                .and_modify(|owner| {
                    if owner.is_some_and(|owner| owner != *terminal_id) {
                        *owner = None;
                    }
                })
                .or_insert(Some(*terminal_id));
        }
        let mut by_workspace = BTreeMap::<Box<str>, Vec<(TerminalId, u32, &'a str)>>::new();
        for (terminal_id, pid, native) in candidates {
            if native_owners.get(native) != Some(&Some(terminal_id)) {
                continue;
            }
            let Some(open) = self.by_id.get(&terminal_id) else {
                continue;
            };
            by_workspace
                .entry(open.workspace.as_str().into())
                .or_default()
                .push((terminal_id, pid, native));
        }

        let mut conflicts = Vec::new();
        let mut any_changed = false;
        for (workspace, batch) in by_workspace {
            let requested = batch
                .iter()
                .map(|(terminal_id, _pid, native)| {
                    (*terminal_id, provider.as_str(), *native, workspace.as_ref())
                })
                .collect::<Vec<_>>();
            let changed = match claims.bind_terminal_natives(&requested) {
                Ok(changed) => changed,
                Err(error) => {
                    conflicts.extend(batch.iter().map(|(_, pid, _)| (*pid, error)));
                    continue;
                }
            };
            for ((terminal_id, pid, native), changed) in batch.into_iter().zip(changed) {
                if let Some(open) = self.by_id.get_mut(&terminal_id) {
                    open.native_process_pid = Some(pid);
                    if changed {
                        let previous = open.native.replace(native.into());
                        if let Some(previous) = previous {
                            let key = (provider, previous);
                            if self.by_conversation.get(&key) == Some(&terminal_id) {
                                self.by_conversation.remove(&key);
                            }
                        }
                    }
                }
                if !changed {
                    continue;
                }
                self.by_conversation
                    .insert((provider, native.into()), terminal_id);
                any_changed = true;
            }
        }
        if any_changed {
            self.publish_change();
        }
        conflicts
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
                native_process_pid: None,
                opened_at_ms: WallMs::now().as_millis(),
                generation: 1,
                stopping: false,
                origin: TerminalOrigin::Owned,
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
        self.brokered_shells.retain(|_, open| *open != id);
        if removed.is_some() {
            self.publish_change();
        }
        removed
    }

    /// Whether any hosted terminal already runs on this process. A mirror is never opened for a process the
    /// daemon already hosts as its own PTY child.
    pub(crate) fn hosts_pid(&self, pid: u32) -> bool {
        self.by_id.values().any(|open| open.terminal.pid() == pid)
    }

    /// Register a terminal renderer onto a process some other owner started.
    ///
    /// The caller reserves a terminal-surface claim before reaching this insertion. That claim does not own the
    /// external conversation, but it does make this the only central renderer across Runtime generations. Filed by
    /// `(provider, native)` so the sidebar row binds and a click attaches here, and by pid so a second observation
    /// does not open a second local renderer.
    fn insert_external(
        &mut self,
        id: TerminalId,
        provider: ProviderId,
        native: &str,
        terminal: Terminal,
        workspace: AbsPath,
        origin: TerminalOrigin,
    ) {
        self.by_id.insert(
            id,
            Open {
                provider,
                terminal,
                workspace,
                native: Some(native.into()),
                native_process_pid: None,
                opened_at_ms: WallMs::now().as_millis(),
                generation: 1,
                stopping: false,
                origin,
            },
        );
        self.by_conversation.insert((provider, native.into()), id);
        self.publish_change();
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// File a terminal the transparent shim opened under every process above the shim (the invoking shell is
    /// among them), and hand back every observed mirror of one of those shells: the shim's own terminal is the one
    /// row for that command generation.
    fn brokered_by_shell(&mut self, id: TerminalId, ancestors: &[u32]) -> Vec<HostedTerminal> {
        for ancestor in ancestors {
            self.brokered_shells.insert(*ancestor, id);
        }
        self.observed_by_shell(ancestors)
    }

    fn observed_by_shell(&self, shells: &[u32]) -> Vec<HostedTerminal> {
        self.by_id
            .iter()
            .filter(|(_, open)| match &open.origin {
                TerminalOrigin::ObservedMirror(owner) => {
                    owner.shell_pid.is_some_and(|pid| shells.contains(&pid))
                }
                TerminalOrigin::Owned
                | TerminalOrigin::ConsoleMirror
                | TerminalOrigin::OfficialAttach(_) => false,
            })
            .map(|(id, open)| HostedTerminal::from_open(*id, open))
            .collect()
    }

    fn observed_by_feeder(
        &self,
        feeder: ConnectionToken,
        terminal_key: &str,
    ) -> Vec<HostedTerminal> {
        self.by_id
            .iter()
            .filter(|(_, open)| match &open.origin {
                TerminalOrigin::ObservedMirror(owner) => {
                    owner.feeder == feeder && owner.terminal_key == terminal_key
                }
                TerminalOrigin::Owned
                | TerminalOrigin::ConsoleMirror
                | TerminalOrigin::OfficialAttach(_) => false,
            })
            .map(|(id, open)| HostedTerminal::from_open(*id, open))
            .collect()
    }

    /// The observed mirror `id` if `feeder` is the connection feeding it.
    fn observed_fed_by(&self, feeder: ConnectionToken, id: TerminalId) -> Option<HostedTerminal> {
        let open = self.by_id.get(&id)?;
        match &open.origin {
            TerminalOrigin::ObservedMirror(owner) if owner.feeder == feeder => {
                Some(HostedTerminal::from_open(id, open))
            }
            TerminalOrigin::ObservedMirror(_)
            | TerminalOrigin::Owned
            | TerminalOrigin::ConsoleMirror
            | TerminalOrigin::OfficialAttach(_) => None,
        }
    }

    /// Every observed mirror `feeder` feeds.
    fn observed_all_by(&self, feeder: ConnectionToken) -> Vec<HostedTerminal> {
        self.by_id
            .iter()
            .filter(|(_, open)| match &open.origin {
                TerminalOrigin::ObservedMirror(owner) => owner.feeder == feeder,
                TerminalOrigin::Owned
                | TerminalOrigin::ConsoleMirror
                | TerminalOrigin::OfficialAttach(_) => false,
            })
            .map(|(id, open)| HostedTerminal::from_open(*id, open))
            .collect()
    }

    /// Register a renderer fed by the window that owns the terminal. No native identity is known yet, so the row
    /// binds to no conversation until a later observation names one.
    fn insert_observed(
        &mut self,
        id: TerminalId,
        provider: ProviderId,
        terminal: Terminal,
        workspace: AbsPath,
        owner: ObservedOwner,
    ) {
        self.by_id.insert(
            id,
            Open {
                provider,
                terminal,
                workspace,
                native: None,
                native_process_pid: None,
                opened_at_ms: WallMs::now().as_millis(),
                generation: 1,
                stopping: false,
                origin: TerminalOrigin::ObservedMirror(Box::new(owner)),
            },
        );
        self.publish_change();
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

    /// Publish a descriptor change after control of a terminal moved to another view or was released, so every
    /// index reader sees the transfer in order.
    pub(crate) fn publish_control_change(&self) {
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
/// How long a hosted terminal must sit with no viewer and no output before a draining generation closes it.
///
/// A viewer that drops and reconnects (a window reload, a transport blip) must not lose its conversation, so
/// the terminal is given this grace with nobody watching before it is let go. Fifteen seconds is long enough
/// to cover a reconnect and short enough that an update's old generations do not pile up for hours.
const DRAINING_IDLE_GRACE_MS: u64 = 15_000;

/// Close the terminals a draining generation is only keeping alive out of habit, so it can finish.
///
/// A draining generation exits when it has no live work, and an open terminal counts as work. The current
/// generation keeps a viewerless terminal so a window or a phone can reattach to the one session; a draining
/// generation cannot be that home (its store has moved), so a conversation nobody is watching and that is not
/// writing is closed here. The provider keeps the conversation, so the next window resumes it in the current
/// generation. This is why old generations used to linger for hours holding idle sessions (operator,
/// 2026-08-29). Only a terminal with no viewer and no output for the grace is closed; one a person is watching,
/// or one a turn is still writing to, is left alone.
///
/// A mirror is let go, never killed. Its process belongs to whoever started it (the operator's Claude Code
/// window, another tool), and killing it ended the operator's own sessions with `0xC0000001` at every Runtime
/// update, minutes after each new generation started (five times on 2026-08-29, two sessions at once each
/// time). Releasing the mirror ends only its console helper; the current generation observes the still-running
/// process again and mirrors it afresh.
pub(crate) async fn close_idle_while_draining(composed: &Arc<Composed>) {
    close_idle_at(composed, WallMs::now().as_millis(), DRAINING_IDLE_GRACE_MS).await;
}

#[cfg(test)]
pub(crate) async fn close_idle_now_for_tests(composed: &Arc<Composed>) {
    close_idle_at(composed, WallMs::now().as_millis(), 0).await;
}

async fn close_idle_at(composed: &Arc<Composed>, now: u64, idle_grace_ms: u64) {
    let candidates: Vec<TerminalId> = {
        let terminals = composed.terminals.lock().await;
        terminals
            .hosted_all()
            .into_iter()
            .map(|hosted| hosted.id)
            .collect()
    };
    for id in candidates {
        // Revalidate and mark stopping under the terminal-table lock. `attach_current` takes the same lock through
        // snapshot subscription, so either the new receiver exists and keeps this renderer or stopping wins and the
        // attach observes TerminalGone. Keep the table slot and live claim until the exit observer sees the process
        // end; otherwise a slow exit could temporarily exceed the process and memory ceilings with a replacement.
        let retired = {
            let mut terminals = composed.terminals.lock().await;
            terminals.hosted(id).and_then(|hosted| {
                if hosted.stopping {
                    return None;
                }
                let quiet_since = hosted
                    .terminal
                    .wrote_at()
                    .map_or(hosted.opened_at_ms, WallMs::as_millis);
                let action = drain_action(
                    &hosted.origin,
                    hosted.terminal.viewer_count(),
                    now.saturating_sub(quiet_since),
                    idle_grace_ms,
                );
                if action == DrainAction::Keep {
                    None
                } else {
                    terminals.mark_stopping(id).then_some((hosted, action))
                }
            })
        };
        let Some((hosted, action)) = retired else {
            continue;
        };
        match action {
            DrainAction::Kill => drop(hosted.terminal.kill()),
            DrainAction::Release => hosted.terminal.release(),
            DrainAction::Keep => {}
        }
    }
}

/// What a draining generation does with one of its terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainAction {
    /// Watched, or still writing: it is live work and the generation waits for it.
    Keep,
    /// A mirror nobody watches: forget it; the process it joined is not the Runtime's to end.
    Release,
    /// A process this Runtime started that nobody watches and that is quiet: end it so the generation can exit.
    Kill,
}

fn drain_action(
    origin: &TerminalOrigin,
    viewers: usize,
    quiet_for_ms: u64,
    idle_grace_ms: u64,
) -> DrainAction {
    if viewers > 0 || quiet_for_ms < idle_grace_ms {
        return DrainAction::Keep;
    }
    match origin {
        TerminalOrigin::ConsoleMirror | TerminalOrigin::ObservedMirror(_) => DrainAction::Release,
        TerminalOrigin::Owned | TerminalOrigin::OfficialAttach(_) => DrainAction::Kill,
    }
}

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
#[expect(
    clippy::too_many_arguments,
    reason = "one open binds provider, native identity, folder, geometry and program, and bundling them would only rename the list"
)]
pub(crate) async fn open_hosted(
    composed: &Arc<Composed>,
    id: ProviderId,
    native: Option<&str>,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
    prepared_program: Option<runtrol_childproc::Program>,
    holder_known: bool,
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
        holder_known,
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
        // An exact invocation from a broker names its own conversation and nobody asked the service about it,
        // so the unnamed-terminal guard stays in force here.
        false,
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
    holder_known: bool,
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
        holder_known,
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

/// Open a mirror of a process the daemon did not start, and attach the requesting viewer.
///
/// This is the door for "a session started anywhere is still one session, streamed to every viewer": the
/// process keeps running wherever it was started (another window, another app, an older Runtime generation
/// that no longer answers), and a helper joins its console so every Runtrol window sees the same screen and
/// can type into it. The row becomes a hosted one, so a click attaches rather than resuming a second copy.
///
/// It opens nothing new for the conversation. It does reserve the one terminal-surface claim while the mirror is
/// live: that claim owns no transcript or conversation, but it prevents another Runtime generation from allocating a
/// second renderer for the same external owner. Observation alone never reaches this function. The first viewer
/// allocates the helper, screen model and output ring, and later viewers attach to those same bounded objects.
pub(crate) async fn open_console_mirror(
    composed: &Arc<Composed>,
    provider: ProviderId,
    native: &str,
    pid: u32,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    let helper = runtrol_childproc::console_mirror::helper_program()
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
    if let Some(existing) = composed.terminals.lock().await.open_for(provider, native) {
        if existing.workspace != cwd {
            return Err(TerminalClaimError::WorkspaceConflict.into());
        }
        let attachment = existing.terminal.attach().await;
        return Ok((existing.id, existing.terminal, attachment));
    }
    // A session another Runtime generation already hosts is already a conversation, not a new one. Its terminal
    // reaches this window through the fleet. The reservation also covers an external mirror already hosted by a
    // peer, so two generations cannot allocate two helpers, rings, and screen models for one owner.
    let terminal_id = TerminalId::now();
    let reservation = match composed.native_claims.reserve_terminal(
        terminal_id,
        provider.as_str(),
        Some(native),
        cwd.as_str(),
        false,
    )? {
        TerminalClaimAdmission::Join(existing) => {
            let (hosted, attachment) = attach_current(composed, existing)
                .await
                .map_err(TerminalOpenError::Provider)?;
            return Ok((hosted.id, hosted.terminal, attachment));
        }
        TerminalClaimAdmission::Reserved(reservation) => reservation,
    };
    let (terminal, open) = {
        let mut terminals = composed.terminals.lock().await;
        if let Some(existing) = terminals.open_for(provider, native) {
            if existing.workspace != cwd {
                return Err(TerminalClaimError::WorkspaceConflict.into());
            }
            let attachment = existing.terminal.attach().await;
            return Ok((existing.id, existing.terminal, attachment));
        }
        if terminals.hosts_pid(pid) {
            return Err(TerminalOpenError::Provider(
                "the selected process is already hosted without this native binding".to_owned(),
            ));
        }
        if terminals.len() >= MAX_HOSTED_TERMINALS {
            return Err(TerminalOpenError::NoRoom {
                held: terminals.len(),
                limit: MAX_HOSTED_TERMINALS,
            });
        }
        let terminal = Terminal::mirror(&helper, pid, runtrol_childproc::PtySize { cols, rows })
            .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        terminals.insert_external(
            terminal_id,
            provider,
            native,
            terminal.clone(),
            cwd,
            TerminalOrigin::ConsoleMirror,
        );
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

/// Open the provider's own TUI attachment client for a conversation another process already owns.
///
/// The attachment is created only when a viewer opens the row. A live conversation with no Runtrol viewer
/// therefore costs no renderer process, screen model, or output ring. The provider roster must have reported an
/// official peer endpoint before the caller reaches this function, and the manifest declares only the exact
/// commands that reach and stop it.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one official attachment keeps reservation, provider declaration, exact argv, renderer insertion and claim commit in one auditable boundary"
)]
pub(crate) async fn open_official_attach(
    composed: &Arc<Composed>,
    provider: ProviderId,
    native: &str,
    attach_target: &runtrol_provider::NativeTerminalTarget,
    cwd: AbsPath,
    cols: u16,
    rows: u16,
    program: runtrol_childproc::Program,
) -> Result<(TerminalId, Terminal, Attachment), TerminalOpenError> {
    if let Some(existing) = composed.terminals.lock().await.open_for(provider, native) {
        if existing.workspace != cwd {
            return Err(TerminalClaimError::WorkspaceConflict.into());
        }
        let attachment = existing.terminal.attach().await;
        return Ok((existing.id, existing.terminal, attachment));
    }
    let declared = composed
        .registry
        .get(provider)
        .ok_or_else(|| TerminalOpenError::Provider(format!("no provider called {provider}")))?;
    let tui = declared.manifest.tui.as_ref().ok_or_else(|| {
        TerminalOpenError::Provider(format!("{provider} declares no terminal interface"))
    })?;
    if tui.attach.is_empty() || tui.stop.is_empty() {
        return Err(TerminalOpenError::Provider(format!(
            "{provider} declares no complete official live terminal attachment"
        )));
    }
    // The provider process remains the conversation owner. This claim owns only the shared terminal surface, so a
    // concurrent open or a successor Runtime generation joins the indexed renderer instead of allocating another
    // attachment client, output ring, and screen for the same owner.
    let terminal_id = TerminalId::now();
    let reservation = match composed.native_claims.reserve_terminal(
        terminal_id,
        provider.as_str(),
        Some(native),
        cwd.as_str(),
        false,
    )? {
        TerminalClaimAdmission::Join(existing) => {
            let (hosted, attachment) = attach_current(composed, existing)
                .await
                .map_err(TerminalOpenError::Provider)?;
            return Ok((hosted.id, hosted.terminal, attachment));
        }
        TerminalClaimAdmission::Reserved(reservation) => reservation,
    };
    let arguments = tui
        .attach
        .iter()
        .map(ToString::to_string)
        .chain(std::iter::once(attach_target.as_str().to_owned()))
        .collect();
    let env = tui
        .env
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    let env_unset = tui
        .env_unset
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let stop = OfficialStop {
        program: program.clone(),
        arguments: tui
            .stop
            .iter()
            .map(ToString::to_string)
            .chain(std::iter::once(attach_target.as_str().to_owned()))
            .collect(),
        cwd: cwd.clone(),
        env: env.clone(),
        env_unset: env_unset.clone(),
    };
    let (terminal, open) = {
        let mut terminals = composed.terminals.lock().await;
        if let Some(existing) = terminals.open_for(provider, native) {
            if existing.workspace != cwd {
                return Err(TerminalClaimError::WorkspaceConflict.into());
            }
            let attachment = existing.terminal.attach().await;
            return Ok((existing.id, existing.terminal, attachment));
        }
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
            env,
            env_unset,
            size: runtrol_childproc::PtySize { cols, rows },
        })
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        terminals.insert_external(
            terminal_id,
            provider,
            native,
            terminal.clone(),
            cwd,
            TerminalOrigin::OfficialAttach(Box::new(stop)),
        );
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

/// The one row for a brokered command: the shim's own terminal. Any mirror a window opened for the same shell
/// before the shim reached the Runtime is retired now, and later mirror opens for that shell are refused.
pub(crate) async fn brokered_by_shell(composed: &Arc<Composed>, id: TerminalId, ancestors: &[u32]) {
    let retired = composed
        .terminals
        .lock()
        .await
        .brokered_by_shell(id, ancestors);
    for mirror in retired {
        retire_observed(composed, &mirror).await;
    }
}

async fn retire_observed(composed: &Arc<Composed>, mirror: &HostedTerminal) {
    // ok: a mirror that already ended has nothing left to end; the table removal below is what matters.
    drop(mirror.terminal.end_feed(None));
    forget(composed, mirror.id).await;
}

/// Open a mirror fed by the VS Code window that owns and observes a terminal (`docs/vscodeSurface.md`,
/// observed mirror). The Runtime spawns nothing: the window sends the raw bytes shell integration handed it,
/// and from here on the terminal is a hosted one for every reader. A second open for the same observed terminal
/// replaces the first (one row per command generation); a shell the transparent shim already brokered is refused,
/// because the shim's own terminal is that row.
pub(crate) async fn open_observed_mirror(
    composed: &Arc<Composed>,
    feeder: ConnectionToken,
    window_session_id: String,
    params: runtrol_runtime_protocol::WindowMirrorOpenParams,
) -> Result<TerminalId, TerminalOpenError> {
    let provider = ProviderId::parse(params.provider_id.as_str())
        .map_err(|_| TerminalOpenError::Provider("the provider identity is invalid".to_owned()))?;
    if composed.registry.get(provider).is_none() {
        return Err(TerminalOpenError::Provider(format!(
            "no provider called {provider}"
        )));
    }
    let cwd = match AbsPath::canonicalize(&params.cwd) {
        Ok(cwd) if cwd.as_std_path().is_dir() => cwd,
        Ok(_) | Err(_) => {
            return Err(TerminalOpenError::Provider(
                "the observed terminal's working directory is not an existing directory".to_owned(),
            ));
        }
    };
    let replaced = {
        let terminals = composed.terminals.lock().await;
        if let Some(shell_pid) = params.process_id
            && terminals.brokered_shells.contains_key(&shell_pid)
        {
            return Err(TerminalOpenError::AlreadyBrokered);
        }
        terminals.observed_by_feeder(feeder, &params.terminal_key)
    };
    for previous in replaced {
        retire_observed(composed, &previous).await;
    }
    let terminal_id = TerminalId::now();
    let (terminal, open) = {
        let mut terminals = composed.terminals.lock().await;
        if terminals.len() >= MAX_HOSTED_TERMINALS {
            return Err(TerminalOpenError::NoRoom {
                held: terminals.len(),
                limit: MAX_HOSTED_TERMINALS,
            });
        }
        let terminal = Terminal::fed(
            params.process_id.unwrap_or(0),
            runtrol_childproc::PtySize {
                cols: params.geometry.columns,
                rows: params.geometry.rows,
            },
        )
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))?;
        terminals.insert_observed(
            terminal_id,
            provider,
            terminal.clone(),
            cwd,
            ObservedOwner {
                window_session_id,
                terminal_key: params.terminal_key,
                feeder,
                shell_pid: params.process_id,
            },
        );
        (terminal, terminals.len())
    };
    composed
        .open_terminals
        .store(open, std::sync::atomic::Ordering::Release);
    forget_on_exit(Arc::clone(composed), terminal_id, &terminal);
    Ok(terminal_id)
}

/// One chunk from the feeding window into its mirror.
pub(crate) async fn feed_observed_mirror(
    composed: &Arc<Composed>,
    feeder: ConnectionToken,
    id: TerminalId,
    bytes: Vec<u8>,
) -> Result<(), TerminalOpenError> {
    let mirror = composed
        .terminals
        .lock()
        .await
        .observed_fed_by(feeder, id)
        .ok_or(TerminalOpenError::NotFedByCaller)?;
    mirror
        .terminal
        .feed(bytes)
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))
}

/// The feeding window says the observed command ended.
pub(crate) async fn end_observed_mirror(
    composed: &Arc<Composed>,
    feeder: ConnectionToken,
    id: TerminalId,
    exit_code: Option<i32>,
) -> Result<(), TerminalOpenError> {
    let mirror = composed
        .terminals
        .lock()
        .await
        .observed_fed_by(feeder, id)
        .ok_or(TerminalOpenError::NotFedByCaller)?;
    mirror
        .terminal
        .end_feed(exit_code)
        .map_err(|error| TerminalOpenError::Provider(error.to_string()))
}

/// The feeding connection went away: every mirror it fed ends, the way a closed window ends its terminals.
pub(crate) async fn end_observed_mirrors_of(composed: &Arc<Composed>, feeder: ConnectionToken) {
    let fed = composed.terminals.lock().await.observed_all_by(feeder);
    for mirror in fed {
        retire_observed(composed, &mirror).await;
    }
}

/// Stop the live conversation behind a terminal, not merely its renderer.
///
/// Owned PTYs and console mirrors already point at the owner process. An official attachment points at a
/// presentation client, so its paired provider command stops the owner first and the renderer is then released.
pub(crate) async fn stop_hosted(hosted: &HostedTerminal) -> Result<(), String> {
    match &hosted.origin {
        TerminalOrigin::Owned | TerminalOrigin::ConsoleMirror => {
            hosted.terminal.kill().map_err(|error| error.to_string())
        }
        TerminalOrigin::OfficialAttach(stop) => {
            run_official_stop(stop).await?;
            hosted.terminal.kill().map_err(|error| error.to_string())
        }
        TerminalOrigin::ObservedMirror(_) => {
            Err("the window that owns this terminal stops it".to_owned())
        }
    }
}

async fn run_official_stop(stop: &OfficialStop) -> Result<(), String> {
    const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    let child = runtrol_childproc::PtyChild::spawn(runtrol_childproc::PtySpawn {
        program: &stop.program,
        arguments: &stop.arguments,
        cwd: &stop.cwd,
        env: &stop.env,
        env_unset: &stop.env_unset,
        size: runtrol_childproc::PtySize { cols: 80, rows: 24 },
    })
    .map_err(|error| format!("starting the provider's official stop command: {error}"))?;
    let reader = child
        .reader()
        .map_err(|error| format!("draining the provider's official stop command: {error}"))?;
    let draining = std::thread::Builder::new()
        .name("runtrol-official-stop-drain".to_owned())
        .spawn(move || {
            use std::io::Read as _;
            let mut reader = reader;
            let mut buffer = [0_u8; 4096];
            while reader.read(&mut buffer).is_ok_and(|read| read != 0) {}
        })
        .map_err(|error| format!("draining the provider's official stop output: {error}"))?;
    let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
    let code = loop {
        match child.try_wait() {
            Ok(Some(code)) => break code,
            Ok(None) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Ok(None) => {
                drop(child.kill());
                return Err("the provider's official stop command exceeded 10 seconds".to_owned());
            }
            Err(error) => {
                return Err(format!(
                    "waiting for the provider's official stop command: {error}"
                ));
            }
        }
    };
    child.finish();
    drop(draining.join());
    if code == 0 {
        Ok(())
    } else {
        Err(format!(
            "the provider's official stop command exited with code {code}"
        ))
    }
}

/// Attach without resizing. Public attach never grants geometry authority implicitly.
pub(crate) async fn attach_current(
    composed: &Composed,
    id: TerminalId,
) -> Result<(HostedTerminal, Attachment), String> {
    let terminals = composed.terminals.lock().await;
    let hosted = terminals
        .hosted(id)
        .ok_or_else(|| format!("no open terminal {id}"))?;
    if hosted.stopping {
        return Err(format!("terminal {id} is stopping"));
    }
    // Keep the table lock until `Terminal::attach` has subscribed under the terminal state lock. Draining takes the
    // same table lock before testing `viewer_count`, which makes attach versus retirement one exact ordering point.
    let attachment = hosted.terminal.attach().await;
    drop(terminals);
    Ok((hosted, attachment))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    const LAUNCHER_PROBE_ENV: &str = "RUNTROL_TERMINAL_LAUNCHER_PROBE";
    const DESCENDANT_PROBE_ENV: &str = "RUNTROL_TERMINAL_DESCENDANT_PROBE";
    const DESCENDANT_PID_MARKER: &str = "RUNTROL_DESCENDANT_PID=";

    struct OwnedTerminals(Vec<Terminal>);

    impl Drop for OwnedTerminals {
        fn drop(&mut self) {
            for terminal in &self.0 {
                drop(terminal.kill());
            }
        }
    }

    async fn descendant_pid(terminal: &Terminal) -> u32 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let attachment = terminal.attach().await;
            let screen = String::from_utf8_lossy(&attachment.snapshot);
            if let Some(tail) = screen.split(DESCENDANT_PID_MARKER).nth(1) {
                let digits = tail
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>();
                if let Ok(pid) = digits.parse() {
                    return pid;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the launcher did not publish its descendant PID: {screen:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn open_launcher_terminal(
        terminals: &mut Terminals,
        claims: &crate::native_claims::NativeLiveClaimRegistry,
        workspace: &AbsPath,
        provider: ProviderId,
        program: &runtrol_childproc::Program,
    ) -> (Terminal, u32) {
        let terminal_id = TerminalId::now();
        let TerminalClaimAdmission::Reserved(reservation) = claims
            .reserve_terminal(
                terminal_id,
                provider.as_str(),
                None,
                workspace.as_str(),
                false,
            )
            .expect("the fresh terminal reserves its claim")
        else {
            panic!("a fresh terminal unexpectedly joined another terminal");
        };
        let terminal = Terminal::open(&TerminalLaunch {
            program,
            arguments: vec![
                "--exact".to_owned(),
                "terminal_surface::tests::launcher_probe_helper".to_owned(),
                "--nocapture".to_owned(),
            ],
            cwd: workspace,
            env: vec![(LAUNCHER_PROBE_ENV.to_owned(), "1".to_owned())],
            env_unset: Vec::new(),
            size: runtrol_childproc::PtySize {
                cols: 120,
                rows: 30,
            },
        })
        .expect("the launcher terminal opens");
        terminals.insert(
            terminal_id,
            provider,
            None,
            terminal.clone(),
            workspace.clone(),
            None,
        );
        reservation.commit().expect("the terminal claim commits");
        let descendant = descendant_pid(&terminal).await;
        (terminal, descendant)
    }

    fn fixture_activity(
        identities: &[(&str, u32, u32)],
        first_native: &str,
    ) -> runtrol_provider::NativeProcessActivity {
        let live = if first_native == "native-first" {
            vec!["native-first", "native-second"]
        } else {
            vec!["native-first", first_native, "native-second"]
        };
        runtrol_provider::NativeProcessActivity {
            live: live
                .into_iter()
                .map(|native| {
                    runtrol_provider::NativeSessionId::new(native)
                        .expect("the fixture native identity is valid")
                })
                .collect(),
            active: Vec::new(),
            processes: identities
                .iter()
                .enumerate()
                .map(|(index, (_native, _root, descendant))| {
                    runtrol_provider::NativeProcessBinding {
                        pid: *descendant,
                        native: runtrol_provider::NativeSessionId::new(if index == 0 {
                            first_native
                        } else {
                            "native-second"
                        })
                        .expect("the fixture process identity is valid"),
                        cwd: None,
                        terminal_access: runtrol_provider::NativeTerminalAccess::Unavailable,
                    }
                })
                .collect(),
        }
    }

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
    fn composed_for(name: &str) -> (Arc<Composed>, String) {
        let root = std::env::temp_dir().join(format!("runtrol-terminal-surface-{name}"));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        let text = root
            .to_str()
            .expect("the temporary path is UTF-8")
            .to_owned();
        let composed =
            Composed::for_tests(&text, runtrol_drivers::builtin()).expect("a fresh home composes");
        (Arc::new(composed), text)
    }

    async fn gone(composed: &Arc<Composed>, id: TerminalId) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if composed.terminals.lock().await.hosted(id).is_none() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    fn mirror_params(
        cwd: &str,
        key: &str,
        provider: &str,
        process_id: Option<u32>,
    ) -> runtrol_runtime_protocol::WindowMirrorOpenParams {
        runtrol_runtime_protocol::WindowMirrorOpenParams {
            terminal_key: key.to_owned(),
            execution_id: format!("e-{key}"),
            provider_id: runtrol_runtime_protocol::ProviderId::new(provider),
            command_line: provider.to_owned(),
            cwd: cwd.to_owned(),
            process_id,
            geometry: runtrol_runtime_protocol::TerminalGeometry {
                columns: 80,
                rows: 24,
            },
        }
    }

    /// An observed mirror is a hosted terminal fed by one connection: its viewers get exactly the fed bytes,
    /// another connection cannot feed it, and the transparent shim's brokered open of the same shell retires it
    /// and keeps a second mirror of that shell from opening.
    #[tokio::test]
    async fn an_observed_mirror_is_fed_by_its_window_and_yields_to_the_shims_brokered_row() {
        let (composed, home) = composed_for("observed-mirror");
        let feeder = ConnectionToken::next();
        // VS Code reports a shell's folder with a lowercase drive letter on Windows; the mirror's folder must still
        // sit under the root the operator approved with the letter as typed.
        let reported_cwd = {
            let mut text = home.clone();
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_lowercase();
            }
            text
        };
        let params = mirror_params(&reported_cwd, "t1", "claude", Some(4242));
        let id = open_observed_mirror(&composed, feeder, "window-1".to_owned(), params.clone())
            .await
            .expect("the mirror opens");
        let hosted = composed
            .terminals
            .lock()
            .await
            .hosted(id)
            .expect("the mirror is listed");
        let (origin, owner) = hosted.origin.projection();
        assert_eq!(
            origin,
            runtrol_runtime_protocol::TerminalOrigin::ObservedMirror
        );
        let owner = owner.expect("an observed mirror names its window");
        assert_eq!(owner.window_session_id, "window-1");
        assert_eq!(owner.terminal_key, "t1");
        assert_eq!(hosted.terminal.pid(), 4242);
        let approved = AbsPath::new(&home).expect("the scratch home is absolute");
        assert!(
            hosted.workspace.is_under(&approved),
            "{} is not under {}",
            hosted.workspace.as_str(),
            approved.as_str()
        );
        let mut attachment = hosted.terminal.attach().await;
        feed_observed_mirror(&composed, feeder, id, b"hello".to_vec())
            .await
            .expect("the feeder feeds");
        let chunk = tokio::time::timeout(Duration::from_secs(2), attachment.live.recv())
            .await
            .expect("the chunk arrives in time")
            .expect("the ring is live");
        assert_eq!(chunk.bytes.as_ref(), b"hello");
        assert!(matches!(
            feed_observed_mirror(&composed, ConnectionToken::next(), id, vec![1]).await,
            Err(TerminalOpenError::NotFedByCaller)
        ));
        assert!(matches!(
            stop_hosted(&hosted).await,
            Err(message) if message.contains("owns this terminal")
        ));

        // The shim reaches the Runtime under the same shell (behind its launcher): its row wins.
        brokered_by_shell(&composed, TerminalId::now(), &[9001, 4242, 1]).await;
        assert!(gone(&composed, id).await, "the mirror is retired");
        assert!(matches!(
            open_observed_mirror(&composed, feeder, "window-1".to_owned(), params).await,
            Err(TerminalOpenError::AlreadyBrokered)
        ));

        // A mirror of another shell ends with its connection.
        let other = open_observed_mirror(
            &composed,
            feeder,
            "window-1".to_owned(),
            mirror_params(&home, "t2", "codex", None),
        )
        .await
        .expect("a second mirror opens");
        end_observed_mirrors_of(&composed, feeder).await;
        assert!(
            gone(&composed, other).await,
            "the connection's mirrors end with it"
        );
        assert_eq!(
            composed
                .open_terminals
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        drop(composed);
        std::fs::remove_dir_all(&home).expect("remove the scratch home");
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_launcher_terminals_bind_to_their_own_distinct_provider_processes() {
        let executable = std::env::current_exe().expect("the test executable has a path");
        let program = runtrol_childproc::resolve(executable.to_string_lossy().as_ref())
            .expect("the test executable resolves");
        let workspace = AbsPath::canonicalize(
            std::env::current_dir()
                .expect("the test has a current directory")
                .to_string_lossy()
                .as_ref(),
        )
        .expect("the test directory canonicalizes");
        let provider = ProviderId::parse("launcher-fixture").expect("a valid provider identifier");
        let claims = crate::native_claims::NativeLiveClaimRegistry::default();
        let mut terminals = Terminals::default();
        let mut owned = OwnedTerminals(Vec::new());
        let mut identities = Vec::new();

        for native in ["native-first", "native-second"] {
            let (terminal, descendant) =
                open_launcher_terminal(&mut terminals, &claims, &workspace, provider, &program)
                    .await;
            identities.push((native, terminal.pid(), descendant));
            owned.0.push(terminal);
        }

        let [first, second] = identities.as_slice() else {
            panic!("the fixture must own exactly two terminal identities");
        };
        assert_ne!(first.1, second.1);
        assert_ne!(first.2, second.2);
        let tree = runtrol_childproc::ProcessTree::capture()
            .expect("the provider process tree is inspectable");
        for (_native, root, descendant) in &identities {
            assert!(tree.contains(*root, *descendant));
        }
        let initial_activity = fixture_activity(&identities, "native-first");
        assert!(terminals.needs_process_tree(provider, &initial_activity));
        assert!(
            terminals
                .bind_native_processes(
                    &claims,
                    Some(&tree),
                    provider,
                    identities
                        .iter()
                        .map(|(native, _root, descendant)| (*descendant, *native)),
                )
                .is_empty(),
            "both structurally distinct descendants bind without a claim conflict"
        );
        assert!(!terminals.needs_process_tree(provider, &initial_activity));

        for (native, root, _descendant) in &identities {
            let hosted = terminals
                .open_for(provider, native)
                .expect("the native identity finds its hosted terminal");
            assert_eq!(hosted.terminal.pid(), *root);
        }

        // The old identity may remain live in another provider process. The exact PID mapping, rather than the
        // provider-wide live set, detects that this hosted CLI moved to a different conversation and pays for one
        // new ancestry snapshot before rebinding.
        let moved_activity = fixture_activity(&identities, "native-first-moved");
        assert!(terminals.needs_process_tree(provider, &moved_activity));
        assert!(
            terminals
                .bind_native_processes(
                    &claims,
                    Some(&tree),
                    provider,
                    moved_activity
                        .processes
                        .iter()
                        .map(|process| (process.pid, process.native.as_str())),
                )
                .is_empty()
        );
        assert!(terminals.open_for(provider, "native-first").is_none());
        assert_eq!(
            terminals
                .open_for(provider, "native-first-moved")
                .expect("the moved identity reuses the first terminal")
                .terminal
                .pid(),
            first.1
        );
        assert!(!terminals.needs_process_tree(provider, &moved_activity));
        assert_eq!(terminals.hosted_all().len(), 2);
    }

    #[test]
    fn launcher_probe_helper() {
        if std::env::var_os(LAUNCHER_PROBE_ENV).is_none() {
            return;
        }
        let mut child =
            Command::new(std::env::current_exe().expect("the test executable has a path"))
                .args([
                    "--exact",
                    "terminal_surface::tests::descendant_probe_helper",
                    "--nocapture",
                ])
                .env(DESCENDANT_PROBE_ENV, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the provider descendant starts");
        println!("{DESCENDANT_PID_MARKER}{}", child.id());
        std::io::stdout()
            .flush()
            .expect("the descendant identity reaches the PTY");
        std::thread::sleep(Duration::from_secs(15));
        drop(child.kill());
        drop(child.wait());
    }

    #[test]
    fn descendant_probe_helper() {
        if std::env::var_os(DESCENDANT_PROBE_ENV).is_some() {
            std::thread::sleep(Duration::from_secs(15));
        }
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

    #[test]
    fn a_draining_generation_ends_only_the_quiet_unwatched_processes_it_started() {
        use super::{DRAINING_IDLE_GRACE_MS, DrainAction, TerminalOrigin, drain_action};
        let grace = DRAINING_IDLE_GRACE_MS;
        assert_eq!(
            drain_action(&TerminalOrigin::Owned, 0, grace, grace),
            DrainAction::Kill
        );
        assert_eq!(
            drain_action(&TerminalOrigin::Owned, 1, grace, grace),
            DrainAction::Keep
        );
        assert_eq!(
            drain_action(&TerminalOrigin::Owned, 0, grace - 1, grace),
            DrainAction::Keep
        );
        // The operator's own Claude Code process, joined by a mirror: let go, never killed.
        assert_eq!(
            drain_action(&TerminalOrigin::ConsoleMirror, 0, grace, grace),
            DrainAction::Release
        );
        assert_eq!(
            drain_action(&TerminalOrigin::ConsoleMirror, 0, grace * 100, grace),
            DrainAction::Release
        );
        assert_eq!(
            drain_action(&TerminalOrigin::ConsoleMirror, 1, grace, grace),
            DrainAction::Keep
        );
        let official = TerminalOrigin::OfficialAttach(Box::new(super::OfficialStop {
            program: runtrol_childproc::resolve("rustc").expect("the Rust compiler is installed"),
            arguments: Vec::new(),
            cwd: runtrol_provider::AbsPath::canonicalize(
                std::env::current_dir()
                    .expect("the test has a current directory")
                    .to_string_lossy()
                    .as_ref(),
            )
            .expect("the test directory canonicalizes"),
            env: Vec::new(),
            env_unset: Vec::new(),
        }));
        assert_eq!(
            drain_action(&official, 0, grace, grace),
            DrainAction::Kill,
            "draining ends only the attachment renderer, not the external owner"
        );
    }
}
