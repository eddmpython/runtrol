//! Public provider-neutral terminal session DTOs.
//!
//! Terminal bytes remain exact opaque bytes. None of these types gives them message, prompt, reply, or transcript
//! meaning, and no terminal identity is a durable provider conversation identity.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{MutationRequestId, ProviderId};

/// Maximum decoded bytes accepted by one `terminals/write` mutation.
pub const MAX_TERMINAL_WRITE_BYTES: usize = 64 * 1024;

/// Maximum decoded bytes in one live terminal output notification.
pub const MAX_TERMINAL_OUTPUT_BYTES: usize = 4 * 1024;

/// Maximum decoded bytes in one bounded terminal screen snapshot.
pub const MAX_TERMINAL_SCREEN_BYTES: usize = 1024 * 1024;

/// Maximum shared PTY columns accepted by the public terminal surface.
pub const MAX_TERMINAL_COLUMNS: u16 = 500;

/// Maximum shared PTY rows accepted by the public terminal surface.
pub const MAX_TERMINAL_ROWS: u16 = 200;

/// Maximum live terminal descriptors returned by one Runtime generation.
pub const MAX_TERMINAL_INDEX_ITEMS: u16 = 256;

/// Maximum queued terminal output chunks for one viewer before an explicit lag boundary.
pub const MAX_TERMINAL_VIEW_QUEUE_CHUNKS: u16 = 128;

/// One hosted provider TUI process in one Runtime generation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct RuntimeTerminalId(String);

impl RuntimeTerminalId {
    /// Mint a new time-ordered terminal identity.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    /// The canonical lowercase UUID spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeTerminalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RuntimeTerminalId {
    type Err = TerminalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_uuid_v7(value).map(Self)
    }
}

impl Serialize for RuntimeTerminalId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RuntimeTerminalId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One connection-bound terminal output subscription.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct RuntimeTerminalViewId(String);

impl RuntimeTerminalViewId {
    /// Mint a new time-ordered terminal view identity.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    /// The canonical lowercase UUID spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeTerminalViewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for RuntimeTerminalViewId {
    type Err = TerminalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_uuid_v7(value).map(Self)
    }
}

impl Serialize for RuntimeTerminalViewId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RuntimeTerminalViewId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

fn parse_uuid_v7(value: &str) -> Result<String, TerminalIdError> {
    let uuid = Uuid::parse_str(value).map_err(|_| TerminalIdError)?;
    if uuid.get_version_num() != 7 || uuid.hyphenated().to_string() != value {
        return Err(TerminalIdError);
    }
    Ok(value.to_owned())
}

/// A terminal or view identity is not a canonical lowercase UUIDv7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("terminal identity must be a canonical lowercase UUIDv7")]
pub struct TerminalIdError;

/// Shared PTY geometry visible to every attached viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalGeometry {
    /// PTY columns.
    pub columns: u16,
    /// PTY rows.
    pub rows: u16,
}

/// Structural process state without terminal content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalProcessState {
    /// The provider CLI process is live.
    Running,
    /// Runtime accepted a stop request and is waiting for process exit.
    Stopping,
}

/// How the Runtime reaches the process behind a terminal (`docs/terminalSurface.md`, live capture ladder).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalOrigin {
    /// The Runtime started the provider on its own pseudo terminal: the fully supervised tier.
    #[default]
    Owned,
    /// The Runtime started only the provider's official attachment client; the owner is elsewhere.
    OfficialAttach,
    /// A VS Code window observes the terminal through shell integration and feeds its raw execution output here;
    /// input reaches it only through that window and only with explicit authority.
    ObservedMirror,
}

/// One live terminal descriptor visible through an approved root.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDescriptor {
    /// Runtime-local terminal identity.
    pub terminal_id: RuntimeTerminalId,
    /// SHA-256 digest of the Runtime generation that owns the process.
    pub runtime_generation: String,
    /// Opaque provider identity returned by Runtime inventory.
    pub provider_id: ProviderId,
    /// Exact canonical workspace already filtered through the caller's grant.
    pub workspace: String,
    /// The lead that started this worker in the same Runtime generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<RuntimeTerminalId>,
    /// Original approved project for a Core-owned worktree. The exact working directory remains `workspace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    /// Identity of the optional initial courier tell. Its body and current delivery state are absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message_id: Option<String>,
    /// Provider-owned durable identity only when it was known before the terminal opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    /// Structural live process state.
    pub process_state: TerminalProcessState,
    /// Runtime wall time when the process was opened, in Unix milliseconds.
    pub opened_at_ms: u64,
    /// Monotonic process incarnation used to reject stale control acquisition.
    pub terminal_generation: u64,
    /// Current shared PTY geometry.
    pub geometry: TerminalGeometry,
    /// Monotonic count of control transfers and renewals on this terminal: exactly one view holds input and
    /// resize authority at a time, and a view whose lease generation is below this number no longer holds it.
    /// Zero until control was first held.
    #[serde(default)]
    pub control_generation: u64,
    /// Whether some view holds a live control lease right now.
    #[serde(default)]
    pub control_held: bool,
    /// Whether local terminal control enabled courier commands for this live process incarnation.
    #[serde(default)]
    pub dialogue_enabled: bool,
    /// How many views are attached to this terminal right now, across every connection and window. A proved
    /// engine fact (`view_count`): the index changes when a view attaches or ends, so a window can say a
    /// conversation is being watched elsewhere without inferring it from output. An open view never implies
    /// model work.
    #[serde(default)]
    pub viewer_count: u32,
    /// Who owns the process behind this terminal and how the Runtime reaches it.
    #[serde(default)]
    pub origin: TerminalOrigin,
    /// For an observed mirror: the VS Code window that owns the terminal, by its registered session identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_window_session_id: Option<String>,
    /// For an observed mirror: the owner window's key for that terminal in the window registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_terminal_key: Option<String>,
    /// Resident memory of the hosted process in bytes, as the operating system reports it at listing time.
    ///
    /// Present only while the process runs and the operating system answered; absent means not measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
}

/// One root-filtered snapshot from one Runtime generation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalIndexSnapshot {
    /// Live descriptors in stable terminal identity order.
    pub terminals: Vec<TerminalDescriptor>,
    /// Safe structural omissions without unauthorized paths or terminal content.
    pub warnings: Vec<String>,
}

/// Read the visible terminal index in the connected Runtime generation.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListTerminalsParams {}

/// Install one bounded terminal index subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchTerminalIndexParams {}

/// Initial terminal index and the connection-local subscription identity.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchTerminalIndexResult {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Initial root-filtered snapshot.
    pub snapshot: TerminalIndexSnapshot,
}

/// Which provider-owned terminal conversation Runtime should host.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TerminalOpenTarget {
    /// Start a fresh provider TUI without inventing a durable native identity.
    Fresh,
    /// Resume one exact provider-native conversation from Runtime's authorized catalogue.
    Native {
        /// Provider-owned opaque conversation identity.
        native_session_id: String,
        /// Short-lived Runtime proof for this exact authorized catalogue observation.
        adoption_token: String,
    },
}

/// Open a fresh terminal or resume one authorized native conversation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOpenParams {
    /// Caller-minted mutation identity retained across outcome queries, never automatic retries.
    pub request_id: MutationRequestId,
    /// Opaque provider identity returned by Runtime inventory.
    pub provider_id: ProviderId,
    /// Exact workspace path under a current approved root.
    pub workspace: String,
    /// Fresh or exact authorized native resume intent.
    pub target: TerminalOpenTarget,
    /// Initial bounded PTY geometry.
    pub geometry: TerminalGeometry,
}

/// Attach one new view at the terminal's current shared geometry.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalAttachParams {
    /// Exact terminal in the connected Runtime generation.
    pub terminal_id: RuntimeTerminalId,
}

/// A view starts with one bounded screen snapshot, then receives live output notifications.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalViewOpened {
    /// Current root-authorized terminal descriptor.
    pub terminal: TerminalDescriptor,
    /// Connection-bound output view.
    pub view_id: RuntimeTerminalViewId,
    /// Base64 of the current bounded terminal screen, delivered before live bytes.
    pub screen_base64: String,
    /// Whether `screen_base64` is the provider's current screen. False, with an empty screen, when the Runtime's
    /// checkpoint projector was unreachable or reset and the provider has not redrawn since; live bytes still
    /// begin exactly at the boundary, and the provider's next full redraw (a resize provokes one) fills the view.
    #[serde(default = "checkpoint_available_by_default")]
    pub checkpoint_available: bool,
    /// Initial write authority only when `open` was authorized for input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_lease: Option<TerminalControlLease>,
}

/// One renewable write authority for one exact terminal process incarnation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalControlLease {
    /// Opaque unguessable lease identity.
    pub lease_id: String,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Process incarnation observed at acquisition.
    pub terminal_generation: u64,
    /// Monotonic lease generation required on renewal and mutations.
    pub lease_generation: u64,
    /// Wall-clock expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Acquire control only from one exact observed live incarnation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalAcquireControlParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact terminal in the connected Runtime generation.
    pub terminal_id: RuntimeTerminalId,
    /// Process incarnation visible when the user chose the action.
    pub expected_terminal_generation: u64,
}

/// Renew or release one exact terminal control lease generation.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalControlParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
}

/// Send exact caller-owned bytes once under one current terminal control lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWriteParams {
    /// Caller-minted idempotency identity. SDKs never resubmit terminal bytes automatically.
    pub request_id: MutationRequestId,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// Base64 of exact caller-owned bytes.
    pub bytes_base64: String,
}

/// Set shared PTY geometry under one current terminal control lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResizeParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// New bounded shared geometry.
    pub geometry: TerminalGeometry,
}

/// Detach only one connection-bound view without stopping the provider process.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDetachParams {
    /// Exact terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Exact view created on this connection.
    pub view_id: RuntimeTerminalViewId,
}

/// Stop one hosted provider CLI under the exact current lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStopParams {
    /// Caller-minted idempotency identity.
    pub request_id: MutationRequestId,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
}

/// Enable or disable process-local dialogue under the exact current terminal input lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSetDialogueParams {
    /// Caller-minted idempotency identity.
    pub request_id: MutationRequestId,
    /// Exact controlled terminal.
    pub terminal_id: RuntimeTerminalId,
    /// Opaque lease identity returned on acquisition.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// False retires the current dialogue lifetime, including pending messages and calls.
    pub enabled: bool,
}

/// Why a terminal index subscription ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalIndexEndReason {
    /// The integration grant was revoked.
    IntegrationRevoked,
    /// Scope, root, key, or grant generation changed and requires reconnect.
    AuthorityChanged,
    /// The Runtime generation stopped publishing the index.
    RuntimeUnavailable,
}

/// Replace one connection's complete visible terminal index snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalIndexChangedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Complete new root-filtered snapshot.
    pub snapshot: TerminalIndexSnapshot,
}

/// Final typed reason for retiring a terminal index subscription.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalIndexEndedNotification {
    /// Opaque connection-local subscription identity.
    pub subscription_id: String,
    /// Structural end reason.
    pub reason: TerminalIndexEndReason,
}

/// One bounded exact output chunk for a terminal view.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOutputNotification {
    /// Exact connection-bound view.
    pub view_id: RuntimeTerminalViewId,
    /// Monotonic sequence within this view, starting at one after its snapshot.
    pub sequence: u64,
    /// Base64 of exact provider CLI output bytes.
    pub bytes_base64: String,
}

/// Explicit loss boundary followed atomically by one replacement screen snapshot.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalLaggedNotification {
    /// Exact connection-bound view.
    pub view_id: RuntimeTerminalViewId,
    /// Number of broadcast chunks skipped before the replacement snapshot.
    pub lost_chunks: u64,
    /// Base64 of the bounded replacement screen.
    pub screen_base64: String,
    /// Whether the replacement screen is the provider's current screen (see [`TerminalViewOpened`]).
    #[serde(default = "checkpoint_available_by_default")]
    pub checkpoint_available: bool,
    /// Sequence assigned to the next live output chunk.
    pub next_sequence: u64,
}

/// A Runtime built before the field always sent a screen it trusted.
const fn checkpoint_available_by_default() -> bool {
    true
}

/// Provider process exit delivered after preceding output has drained.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalExitedNotification {
    /// Exact connection-bound view.
    pub view_id: RuntimeTerminalViewId,
    /// Provider process exit code as reported by the terminal host.
    pub exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_ids_are_canonical_uuid_v7_values() {
        let terminal = RuntimeTerminalId::now();
        assert_eq!(terminal.as_str().parse(), Ok(terminal.clone()));
        assert!("not-an-id".parse::<RuntimeTerminalId>().is_err());
        assert!(
            Uuid::nil()
                .to_string()
                .parse::<RuntimeTerminalViewId>()
                .is_err()
        );
    }

    #[test]
    fn descriptor_has_no_conversation_content_fields() {
        let descriptor = TerminalDescriptor {
            terminal_id: RuntimeTerminalId::now(),
            runtime_generation: "0".repeat(64),
            provider_id: ProviderId::new("provider"),
            workspace: "/work".to_owned(),
            spawned_by: None,
            project_root: None,
            initial_message_id: None,
            native_session_id: Some("native".to_owned()),
            process_state: TerminalProcessState::Running,
            opened_at_ms: 1,
            terminal_generation: 1,
            geometry: TerminalGeometry {
                columns: 80,
                rows: 24,
            },
            control_generation: 1,
            control_held: true,
            dialogue_enabled: false,
            viewer_count: 1,
            origin: TerminalOrigin::Owned,
            owner_window_session_id: None,
            owner_terminal_key: None,
            memory_bytes: None,
        };
        let value = serde_json::to_value(descriptor).expect("serializable");
        for forbidden in ["title", "prompt", "reply", "transcript", "screenBase64"] {
            assert!(
                value.get(forbidden).is_none(),
                "descriptor exposed {forbidden}"
            );
        }
    }
}
