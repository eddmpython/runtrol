//! Provider-neutral session start, native adoption, and managed resume DTOs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ControlLease, LifecycleState, MutationRequestId, ProviderId, RuntimeSessionId,
    SessionDescriptor,
};

/// Maximum bytes accepted for one provider-owned model selection.
pub const MAX_MODEL_SELECTION_BYTES: usize = 4 * 1024;

/// Maximum bytes accepted for one provider-owned reasoning selection.
pub const MAX_REASONING_SELECTION_BYTES: usize = 4 * 1024;

/// Maximum bytes accepted for one provider-owned permission mode selection.
pub const MAX_PERMISSION_SELECTION_BYTES: usize = 4 * 1024;

/// Whether a newly heated process must be the only writer for its working tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionWorkspaceAccess {
    /// Refuse any overlapping opening, live, or closing writer.
    Exclusive,
    /// The local operator explicitly approved concurrent writers for this operation.
    Shared,
}

/// Start a new provider-native session in one exact authorized workspace.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartSessionParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Opaque provider identity returned by Runtime inventory.
    pub provider_id: ProviderId,
    /// Exact workspace path under a current approved root.
    pub workspace: String,
    /// Writer collision posture for the working tree.
    pub access: SessionWorkspaceAccess,
    /// Exact opaque model choice previously returned by Runtime, or provider default when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Exact opaque reasoning choice returned for the selected model, or provider default when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Exact permission mode to start at, or provider default when absent.
    ///
    /// The value must sit inside the provider's declared switchable vocabulary, the same boundary
    /// `sessions/setMode` enforces, so starting a session can never reach a mode that switching one
    /// could not. Modes that remove safety prompts are outside that vocabulary for every caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

/// Adopt one exact native catalogue observation into Runtime supervision.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdoptNativeSessionParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Opaque provider identity returned with the native catalogue entry.
    pub provider_id: ProviderId,
    /// Provider-owned opaque session identity returned unchanged.
    pub native_session_id: String,
    /// Exact canonical workspace returned with the native catalogue entry.
    pub workspace: String,
    /// Writer collision posture for the working tree.
    pub access: SessionWorkspaceAccess,
    /// Short-lived Runtime proof issued with the authorized native catalogue entry.
    pub adoption_token: String,
}

/// Heat one existing Runtime-managed cold session.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResumeSessionParams {
    /// Caller-minted mutation identity.
    pub request_id: MutationRequestId,
    /// Exact Runtime-managed session.
    pub session_id: RuntimeSessionId,
    /// Lifecycle visible when the caller chose to resume.
    pub expected_lifecycle: LifecycleState,
    /// Lifecycle generation visible when the caller chose to resume.
    pub expected_session_generation: u64,
    /// Exact current managed workspace, reauthorized before provider I/O.
    pub workspace: String,
    /// Writer collision posture for the working tree.
    pub access: SessionWorkspaceAccess,
    /// Exact opaque model choice previously returned by Runtime, or the session's own setting when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Exact opaque reasoning choice returned for the selected model, or the session's own setting when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// One newly supervised or reheated session and its initial controller authority.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOpenResult {
    /// Current structural managed-session descriptor.
    pub session: SessionDescriptor,
    /// Initial short-lived control lease held by the integration that opened it.
    pub control: ControlLease,
}
