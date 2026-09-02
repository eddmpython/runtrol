//! The window registry: one bounded, memory-only entry per VS Code window, bound to the connection that
//! registered it (`runtrol-runtime-protocol::windows`).
//!
//! A window registers under its own session identity and publishes the terminals it observes whenever they
//! change; nothing here polls anything. The entry lives exactly as long as the registering connection: when the
//! connection ends, the entry is dropped and every index reader is told. A new registration under the same window
//! identity (the Extension Host restarted and came back on a new connection) replaces the old entry with a higher
//! registration generation, so there is never a duplicate window and never a stale one that outlives its host.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use runtrol_runtime_protocol::{
    MAX_OBSERVED_TERMINALS, MAX_REGISTERED_WINDOWS, MAX_WINDOW_FOLDERS, MAX_WINDOW_TEXT_CHARS,
    ObservedTerminal, RuntimeErrorKind, WindowDescriptor, WindowIndexSnapshot,
    WindowRegisterParams, WindowRegistration, WindowUpdateParams,
};
use tokio::sync::{Mutex, watch};

/// One public connection, told apart from every other for the life of the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ConnectionToken(u64);

impl ConnectionToken {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Why a registration or an update was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowRegistryFailure {
    pub(crate) kind: RuntimeErrorKind,
    pub(crate) message: &'static str,
}

const fn refused(kind: RuntimeErrorKind, message: &'static str) -> WindowRegistryFailure {
    WindowRegistryFailure { kind, message }
}

struct Registered {
    connection: ConnectionToken,
    descriptor: WindowDescriptor,
}

#[derive(Default)]
struct Registry {
    /// By window session identity, so a window has at most one entry.
    windows: BTreeMap<String, Registered>,
    next_generation: u64,
}

pub(crate) struct WindowRegistry {
    state: Mutex<Registry>,
    changes: watch::Sender<u64>,
}

impl Default for WindowRegistry {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            state: Mutex::new(Registry::default()),
            changes,
        }
    }
}

impl WindowRegistry {
    /// Register a window on `connection`, replacing any earlier entry for the same window.
    pub(crate) async fn register(
        &self,
        connection: ConnectionToken,
        params: WindowRegisterParams,
    ) -> Result<WindowRegistration, WindowRegistryFailure> {
        bounded_text(&params.window_session_id, "the window session identity")?;
        bounded_text(&params.host_generation, "the host generation")?;
        bounded_text(&params.vscode_version, "the VS Code version")?;
        if params.workspace_folders.len() > MAX_WINDOW_FOLDERS {
            return Err(refused(
                RuntimeErrorKind::ResourceExhausted,
                "the window publishes more workspace folders than the registry keeps",
            ));
        }
        for folder in &params.workspace_folders {
            bounded_label(folder, "a workspace folder")?;
        }
        let mut state = self.state.lock().await;
        // The same window again (a restarted host) keeps its slot; a new window needs a free one. A window this
        // connection registered under another identity (a reload changed it) gives its slot up first.
        state.windows.retain(|session, entry| {
            entry.connection != connection || *session == params.window_session_id
        });
        if !state.windows.contains_key(&params.window_session_id)
            && state.windows.len() >= MAX_REGISTERED_WINDOWS
        {
            return Err(refused(
                RuntimeErrorKind::ResourceExhausted,
                "the bounded window registry is full",
            ));
        }
        state.next_generation = state.next_generation.saturating_add(1);
        let registration_generation = state.next_generation;
        // The terminals a restarted host observes are published by its next update; until then the entry says
        // none rather than repeating what the old host said.
        state.windows.insert(
            params.window_session_id.clone(),
            Registered {
                connection,
                descriptor: WindowDescriptor {
                    window_session_id: params.window_session_id,
                    host_generation: params.host_generation,
                    registration_generation,
                    vscode_version: params.vscode_version,
                    workspace_folders: params.workspace_folders,
                    terminals: Vec::new(),
                },
            },
        );
        drop(state);
        self.publish();
        Ok(WindowRegistration {
            registration_generation,
        })
    }

    /// Replace the observed terminals of the window `connection` registered.
    pub(crate) async fn update(
        &self,
        connection: ConnectionToken,
        params: WindowUpdateParams,
    ) -> Result<(), WindowRegistryFailure> {
        if params.terminals.len() > MAX_OBSERVED_TERMINALS {
            return Err(refused(
                RuntimeErrorKind::ResourceExhausted,
                "the window publishes more terminals than the registry keeps",
            ));
        }
        for terminal in &params.terminals {
            bounded_terminal(terminal)?;
        }
        let mut state = self.state.lock().await;
        let Some(entry) = state
            .windows
            .values_mut()
            .find(|entry| entry.connection == connection)
        else {
            return Err(refused(
                RuntimeErrorKind::InvalidRequest,
                "this connection registered no window",
            ));
        };
        if entry.descriptor.terminals == params.terminals {
            return Ok(());
        }
        entry.descriptor.terminals = params.terminals;
        drop(state);
        self.publish();
        Ok(())
    }

    /// The connection ended: whatever it registered is gone with it.
    pub(crate) async fn forget_connection(&self, connection: ConnectionToken) {
        let mut state = self.state.lock().await;
        let before = state.windows.len();
        state
            .windows
            .retain(|_, entry| entry.connection != connection);
        let removed = state.windows.len() != before;
        drop(state);
        if removed {
            self.publish();
        }
    }

    pub(crate) async fn snapshot(&self) -> WindowIndexSnapshot {
        let state = self.state.lock().await;
        let mut windows: Vec<WindowDescriptor> = state
            .windows
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect();
        windows.sort_by_key(|window| window.registration_generation);
        WindowIndexSnapshot { windows }
    }

    /// Wakes on every registration, update, and departure. Readers re-read the snapshot.
    pub(crate) fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn publish(&self) {
        let next = self.changes.borrow().wrapping_add(1);
        self.changes.send_replace(next);
    }
}

/// An identity: never empty, never longer than the registry keeps.
fn bounded_text(text: &str, what: &'static str) -> Result<(), WindowRegistryFailure> {
    if text.is_empty() {
        return Err(refused(
            RuntimeErrorKind::InvalidRequest,
            "a registry identity is empty",
        ));
    }
    bounded_label(text, what)
}

/// A label the window reports as it finds it: a terminal's name is empty until its shell has started (measured
/// 2026-09-02 on a terminal opened from the menu), a command line may be empty at low confidence. Only the
/// length is bounded.
fn bounded_label(text: &str, what: &'static str) -> Result<(), WindowRegistryFailure> {
    if text.chars().count() > MAX_WINDOW_TEXT_CHARS {
        return Err(refused(RuntimeErrorKind::InvalidRequest, what));
    }
    Ok(())
}

fn bounded_terminal(terminal: &ObservedTerminal) -> Result<(), WindowRegistryFailure> {
    bounded_text(&terminal.terminal_key, "a terminal key is too long")?;
    bounded_label(&terminal.name, "a terminal name is too long")?;
    if let Some(cwd) = &terminal.cwd {
        bounded_label(cwd, "a terminal working directory is too long")?;
    }
    if let Some(command) = &terminal.command {
        bounded_text(&command.execution_id, "an execution identity is too long")?;
        bounded_label(&command.command_line, "a command line is too long")?;
        if command.confidence > 2 {
            return Err(refused(
                RuntimeErrorKind::InvalidRequest,
                "a command line confidence is outside 0..=2",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtrol_runtime_protocol::ObservedCommand;

    fn window(session: &str, host: &str) -> WindowRegisterParams {
        WindowRegisterParams {
            window_session_id: session.to_owned(),
            host_generation: host.to_owned(),
            vscode_version: "1.132.1".to_owned(),
            workspace_folders: vec!["C:\\work".to_owned()],
        }
    }

    fn terminal(key: &str, command: Option<&str>) -> ObservedTerminal {
        ObservedTerminal {
            terminal_key: key.to_owned(),
            name: "pwsh".to_owned(),
            process_id: Some(4242),
            shell_integration: true,
            cwd: Some("C:\\work".to_owned()),
            command: command.map(|line| ObservedCommand {
                execution_id: format!("{key}-1"),
                command_line: line.to_owned(),
                confidence: 2,
                started_at_ms: 1,
            }),
        }
    }

    #[tokio::test]
    async fn a_window_has_one_entry_and_a_restarted_host_replaces_it_with_a_higher_generation() {
        let registry = WindowRegistry::default();
        let mut changes = registry.changes();
        let first = ConnectionToken::next();
        let registered = registry
            .register(first, window("w1", "host-a"))
            .await
            .expect("the first registration is accepted");
        registry
            .update(
                first,
                WindowUpdateParams {
                    terminals: vec![terminal("t1", Some("claude"))],
                },
            )
            .await
            .expect("the window publishes its terminals");
        assert!(changes.has_changed().expect("the watch lives"));
        drop(changes.borrow_and_update());
        // The host restarted: a new connection, the same window.
        let second = ConnectionToken::next();
        let again = registry
            .register(second, window("w1", "host-b"))
            .await
            .expect("the same window registers again");
        assert!(again.registration_generation > registered.registration_generation);
        let snapshot = registry.snapshot().await;
        assert_eq!(snapshot.windows.len(), 1, "one window, one entry");
        let only = snapshot.windows.first().expect("one window");
        assert_eq!(only.host_generation, "host-b");
        assert!(
            only.terminals.is_empty(),
            "the old host's terminals are not repeated for the new one"
        );
        // The old connection going away must not remove the new registration.
        registry.forget_connection(first).await;
        assert_eq!(registry.snapshot().await.windows.len(), 1);
        registry.forget_connection(second).await;
        assert!(
            registry.snapshot().await.windows.is_empty(),
            "the entry leaves with its connection"
        );
    }

    #[tokio::test]
    async fn an_update_needs_a_registration_and_an_unchanged_set_publishes_nothing() {
        let registry = WindowRegistry::default();
        let stranger = ConnectionToken::next();
        let refused = registry
            .update(stranger, WindowUpdateParams { terminals: vec![] })
            .await
            .expect_err("a connection that registered nothing cannot update");
        assert_eq!(refused.kind, RuntimeErrorKind::InvalidRequest);
        let connection = ConnectionToken::next();
        registry
            .register(connection, window("w2", "host"))
            .await
            .expect("registered");
        let mut changes = registry.changes();
        drop(changes.borrow_and_update());
        let terminals = vec![terminal("t1", None)];
        registry
            .update(
                connection,
                WindowUpdateParams {
                    terminals: terminals.clone(),
                },
            )
            .await
            .expect("first update");
        assert!(changes.has_changed().expect("the watch lives"));
        drop(changes.borrow_and_update());
        registry
            .update(connection, WindowUpdateParams { terminals })
            .await
            .expect("same update");
        assert!(
            !changes.has_changed().expect("the watch lives"),
            "nothing changed, nothing published"
        );
    }

    #[tokio::test]
    async fn the_registry_is_bounded() {
        let registry = WindowRegistry::default();
        for index in 0..MAX_REGISTERED_WINDOWS {
            registry
                .register(
                    ConnectionToken::next(),
                    window(&format!("w{index}"), "host"),
                )
                .await
                .expect("inside the bound");
        }
        let refused = registry
            .register(ConnectionToken::next(), window("one-too-many", "host"))
            .await
            .expect_err("the bound holds");
        assert_eq!(refused.kind, RuntimeErrorKind::ResourceExhausted);
        let long = "x".repeat(MAX_WINDOW_TEXT_CHARS + 1);
        let refused = registry
            .register(ConnectionToken::next(), window(&long, "host"))
            .await
            .expect_err("an overlong identity is refused");
        assert_eq!(refused.kind, RuntimeErrorKind::InvalidRequest);
        let connection = ConnectionToken::next();
        registry.forget_connection(ConnectionToken(1)).await;
        registry
            .register(connection, window("w0", "host-again"))
            .await
            .expect("a known window still registers when the registry is full");
        let too_many = WindowUpdateParams {
            terminals: (0..=MAX_OBSERVED_TERMINALS)
                .map(|index| terminal(&format!("t{index}"), None))
                .collect(),
        };
        assert_eq!(
            registry
                .update(connection, too_many)
                .await
                .expect_err("too many terminals")
                .kind,
            RuntimeErrorKind::ResourceExhausted
        );
    }
}
