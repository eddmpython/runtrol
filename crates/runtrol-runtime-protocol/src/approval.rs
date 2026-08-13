//! Public structured provider approval vocabulary.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{MutationRequestId, RuntimeSessionId};

/// Current control lease required to inspect pending approvals for one session.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListPendingApprovalsParams {
    /// Exact controlled Runtime session.
    pub session_id: RuntimeSessionId,
    /// Opaque current lease identity.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
}

/// Every provider approval still pending for the exact controlled session.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingApprovalList {
    /// Bounded pending requests in provider order.
    pub approvals: Vec<PendingApproval>,
}

/// One provider-neutral approval request retained by the live driver.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingApproval {
    /// Runtrol-owned approval identity.
    pub approval_id: String,
    /// Structural provider request class.
    pub kind: RuntimeApprovalKind,
    /// Risk computed from the held provider request.
    pub risk: RuntimeApprovalRisk,
    /// Every provider-offered option without silently hiding unavailable choices.
    pub options: Vec<RuntimeApprovalOption>,
    /// Provider-normalized subject that must be presented for meaningful consent.
    pub subject: Value,
    /// Whether Runtime could not bind the complete subject.
    pub subject_incomplete: bool,
    /// Exact subject digest required in the response.
    pub subject_digest: [u8; 32],
    /// Provider request expiry in Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Structural provider approval class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeApprovalKind {
    /// Run a command.
    Command,
    /// Change files.
    FileChange,
    /// Widen permissions.
    Permissions,
    /// Answer a provider question.
    Elicitation,
    /// Reach the network.
    Network,
    /// A structural class unknown to this revision.
    Other,
}

/// Authority class derived from the pending request and selected option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeApprovalRisk {
    /// One action with no standing policy change.
    Low,
    /// Code execution, destructive action, or standing policy change.
    High,
}

/// One provider-offered approval option and its current availability.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeApprovalOption {
    /// Runtrol-owned option identity within this approval.
    pub option_id: u32,
    /// Provider-owned presentation label transported verbatim.
    pub label: String,
    /// Structural effect of choosing the option.
    pub kind: RuntimeApprovalOptionKind,
    /// Safe Runtime-owned reason the current integration cannot choose it.
    pub unavailable: Option<String>,
}

/// Structural effect of one provider-offered option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeApprovalOptionKind {
    /// Allow only this action.
    AllowOnce,
    /// Create a standing allowance.
    AllowAlways,
    /// Reject only this action.
    RejectOnce,
    /// Create a standing rejection.
    RejectAlways,
}

/// Answer one exact provider approval under the current control lease.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RespondApprovalParams {
    /// Caller-minted idempotency identity.
    pub request_id: MutationRequestId,
    /// Exact controlled Runtime session.
    pub session_id: RuntimeSessionId,
    /// Opaque current lease identity.
    pub lease_id: String,
    /// Exact current lease generation.
    pub lease_generation: u64,
    /// Exact pending approval identity.
    pub approval_id: String,
    /// Exact provider-offered option identity.
    pub option_id: u32,
    /// Digest returned by `approvals/listPending`.
    pub subject_digest: [u8; 32],
}
