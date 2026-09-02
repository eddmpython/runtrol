//! The window registry: what every VS Code window tells the Runtime about itself and the terminals it observes.
//!
//! A window registers once per Extension Host activation and then publishes changes as they happen (a terminal
//! opened or closed, shell integration attached, a command started or ended). The Runtime keeps one bounded,
//! memory-only entry per window, bound to the connection that registered it: when that connection ends (the host
//! restarted, the window closed, the machine slept past the transport), the entry is gone with it, and the next
//! registration under the same window identity replaces whatever was there. Other windows read the registry through
//! `windows/list` and `windows/watchIndex`, so a row that belongs to another window can name its owner exactly.
//!
//! Nothing here carries terminal content. A command line is the shell's own record of what was started, the way
//! VS Code's shell integration reports it; output is never registered.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many windows one Runtime remembers at once.
pub const MAX_REGISTERED_WINDOWS: usize = 16;
/// How many observed terminals one window may publish.
pub const MAX_OBSERVED_TERMINALS: usize = 64;
/// How many workspace folders one window may publish.
pub const MAX_WINDOW_FOLDERS: usize = 32;
/// The longest string the registry accepts for an identity, a name, a folder, or a command line.
pub const MAX_WINDOW_TEXT_CHARS: usize = 1024;

/// A window announcing itself: once per Extension Host activation, on the connection it keeps open.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowRegisterParams {
    /// The window's own identity: VS Code's session id, one per window, kept across an Extension Host restart and
    /// renewed by a window reload.
    pub window_session_id: String,
    /// One value per Extension Host activation, so a restarted host is told apart from the one before it.
    pub host_generation: String,
    /// The VS Code version the window runs.
    pub vscode_version: String,
    /// The workspace folders open in the window, as absolute paths.
    pub workspace_folders: Vec<String>,
}

/// The Runtime's record of a registration.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowRegistration {
    /// Monotonic across every registration this Runtime generation accepted; a later registration of the same
    /// window has a higher number, so readers can tell which one is current.
    pub registration_generation: u64,
}

/// The command a shell-integrated terminal is running right now, as VS Code's shell integration reported it.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedCommand {
    /// One value per started execution in the owning window: the command generation.
    pub execution_id: String,
    /// The command line as the shell reported it.
    pub command_line: String,
    /// VS Code's confidence in the command line: 0 low, 1 medium, 2 high.
    pub confidence: u8,
    /// When the execution started, in the window's Unix milliseconds.
    pub started_at_ms: u64,
}

/// One terminal a window observes: an ordinary VS Code terminal, not one the Runtime hosts.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTerminal {
    /// The window's own key for the terminal, stable for the terminal's life in that window.
    pub terminal_key: String,
    /// The terminal's name as VS Code shows it.
    pub name: String,
    /// The shell process id, once VS Code has resolved it; a terminal keeps it across a host restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// Whether VS Code's shell integration is attached, which is what makes command observation possible.
    pub shell_integration: bool,
    /// The shell's current working directory when shell integration reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The command running now, when shell integration reported one that has not ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<ObservedCommand>,
}

/// A window publishing the terminals it observes: the whole set, every time something changed.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowUpdateParams {
    /// Every terminal the window observes right now.
    pub terminals: Vec<ObservedTerminal>,
}

/// One registered window as every reader sees it.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowDescriptor {
    /// The window's identity, as registered.
    pub window_session_id: String,
    /// The Extension Host activation that holds the registration.
    pub host_generation: String,
    /// Which registration this is; higher is newer.
    pub registration_generation: u64,
    /// The VS Code version the window runs.
    pub vscode_version: String,
    /// The workspace folders open in the window.
    pub workspace_folders: Vec<String>,
    /// Every terminal the window published, in the order it published them.
    pub terminals: Vec<ObservedTerminal>,
}

/// Every registered window, in registration order.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowIndexSnapshot {
    /// The registered windows, oldest registration first.
    pub windows: Vec<WindowDescriptor>,
}

/// Read every registered window.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListWindowsParams {}

/// Install one bounded window index subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchWindowIndexParams {}

/// Initial window index and the connection-local subscription identity.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchWindowIndexResult {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Initial snapshot.
    pub snapshot: WindowIndexSnapshot,
}

/// The window index after a registration, an update, or a window going away.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowIndexChangedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Complete new snapshot.
    pub snapshot: WindowIndexSnapshot,
}

/// Final typed reason for retiring a window index subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowIndexEndReason {
    /// The integration's authority changed or was revoked.
    AuthorityChanged,
    /// The Runtime generation is going away.
    RuntimeUnavailable,
}

/// The window index subscription ended.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowIndexEndedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Structural end reason.
    pub reason: WindowIndexEndReason,
}
