//! Stable machine failures for public clients.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A stable public failure category. Clients never branch on prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum RuntimeErrorKind {
    /// No verified Runtime installation is present.
    RuntimeNotInstalled,
    /// The Runtime cannot currently be reached or trusted.
    RuntimeUnavailable,
    /// The client and Runtime share no finalized revision.
    ProtocolIncompatible,
    /// A method was sent before initialization completed.
    NotInitialized,
    /// The connection has no enrolled integration identity.
    Unauthenticated,
    /// The integration is waiting for local approval.
    EnrollmentPending,
    /// The operator denied enrollment.
    EnrollmentDenied,
    /// The integration grant was revoked.
    IntegrationRevoked,
    /// The integration grant lacks the method scope.
    ScopeDenied,
    /// A private local action is required.
    PresenceRequired,
    /// The target is outside the approved project roots.
    RootDenied,
    /// The provider is not currently usable.
    ProviderUnavailable,
    /// The selected negotiated or provider capability is absent.
    CapabilityUnavailable,
    /// The provider no longer accepts the model selection.
    ModelUnavailable,
    /// The provider has no registered official native catalogue.
    NativeCatalogueUnsupported,
    /// The Runtime session does not exist in the caller's grant.
    SessionNotFound,
    /// The requested lifecycle transition conflicts with current state.
    SessionConflict,
    /// Another integration controls the session.
    ControlConflict,
    /// The supplied control lease has expired.
    LeaseExpired,
    /// The working tree already has an incompatible writer.
    WorkspaceConflict,
    /// The pending approval has expired.
    ApprovalExpired,
    /// The chosen approval option is not currently offered.
    ApprovalOptionInvalid,
    /// One mutation ID was reused with different parameters.
    IdempotencyConflict,
    /// A mutation may have happened and cannot be repeated safely.
    OutcomeUnknown,
    /// A bounded resource has reached its advertised limit.
    ResourceExhausted,
    /// The caller exceeded a bounded admission rate.
    RateLimited,
    /// A reconnect cursor is outside bounded replay.
    Gap,
    /// The JSON-RPC request or its closed parameters are invalid.
    InvalidRequest,
    /// The public method name does not exist.
    MethodNotFound,
    /// Runtime failed without exposing private implementation detail.
    Internal,
}

impl RuntimeErrorKind {
    /// Stable lower-camel machine label used on the wire and in local audit metadata.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeNotInstalled => "runtimeNotInstalled",
            Self::RuntimeUnavailable => "runtimeUnavailable",
            Self::ProtocolIncompatible => "protocolIncompatible",
            Self::NotInitialized => "notInitialized",
            Self::Unauthenticated => "unauthenticated",
            Self::EnrollmentPending => "enrollmentPending",
            Self::EnrollmentDenied => "enrollmentDenied",
            Self::IntegrationRevoked => "integrationRevoked",
            Self::ScopeDenied => "scopeDenied",
            Self::PresenceRequired => "presenceRequired",
            Self::RootDenied => "rootDenied",
            Self::ProviderUnavailable => "providerUnavailable",
            Self::CapabilityUnavailable => "capabilityUnavailable",
            Self::ModelUnavailable => "modelUnavailable",
            Self::NativeCatalogueUnsupported => "nativeCatalogueUnsupported",
            Self::SessionNotFound => "sessionNotFound",
            Self::SessionConflict => "sessionConflict",
            Self::ControlConflict => "controlConflict",
            Self::LeaseExpired => "leaseExpired",
            Self::WorkspaceConflict => "workspaceConflict",
            Self::ApprovalExpired => "approvalExpired",
            Self::ApprovalOptionInvalid => "approvalOptionInvalid",
            Self::IdempotencyConflict => "idempotencyConflict",
            Self::OutcomeUnknown => "outcomeUnknown",
            Self::ResourceExhausted => "resourceExhausted",
            Self::RateLimited => "rateLimited",
            Self::Gap => "gap",
            Self::InvalidRequest => "invalidRequest",
            Self::MethodNotFound => "methodNotFound",
            Self::Internal => "internal",
        }
    }
}

/// A public failure with stable machine fields and bounded safe text.
#[derive(Clone, Debug, PartialEq, JsonSchema, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeError {
    /// Stable branch key.
    pub code: RuntimeErrorKind,
    /// Safe presentation text. Clients do not parse it.
    pub message: String,
    /// Whether repeating later may succeed without changing parameters.
    pub retryable: bool,
    /// Optional stable local action name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator_action: Option<String>,
    /// Opaque correlation identifier with no content-derived data.
    pub correlation_id: String,
}

impl RuntimeError {
    /// Construct a safe failure without an operator action.
    #[must_use]
    pub fn plain(
        code: RuntimeErrorKind,
        message: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            operator_action: None,
            correlation_id: correlation_id.into(),
        }
    }
}
