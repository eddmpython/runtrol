//! Closed Mission and Task state machines.

use serde::{Deserialize, Serialize};

/// Durable Mission lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MissionState {
    /// Loaded without authority.
    Draft,
    /// Closed validation passed.
    Validated,
    /// Exact local review approved preparation.
    Ready,
    /// New Task reservations are allowed.
    Running,
    /// New reservations are paused.
    Paused,
    /// An explicit recoverable blocker exists.
    Blocked,
    /// Required Tasks passed and local integration review is pending.
    Integrating,
    /// Integration and its gates passed.
    Completed,
    /// No recovery path remains.
    Failed,
    /// Owned activity was cancelled and reconciled.
    Cancelled,
    /// Immutable terminal summary.
    Archived,
    /// Validation or review rejected the Mission.
    Rejected,
}

impl MissionState {
    /// Whether `next` is one declared transition from this state.
    #[must_use]
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Validated | Self::Rejected)
                | (Self::Validated, Self::Ready | Self::Rejected)
                | (Self::Ready | Self::Paused, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Paused
                        | Self::Blocked
                        | Self::Integrating
                        | Self::Failed
                        | Self::Cancelled
                )
                | (
                    Self::Blocked,
                    Self::Running | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Integrating,
                    Self::Completed | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Completed | Self::Failed | Self::Cancelled,
                    Self::Archived
                )
        )
    }

    /// Whether the Mission can no longer execute work.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Archived | Self::Rejected
        )
    }
}

/// Durable Task lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    /// Dependencies are unresolved.
    Pending,
    /// Dependencies and condition allow reservation.
    Eligible,
    /// All bounded resources were reserved atomically.
    Reserved,
    /// The exact session is ready and waits for local input.
    AwaitingInput,
    /// Reviewed instruction bytes were locally submitted.
    Running,
    /// A provider-native approval is pending.
    AwaitingApproval,
    /// Artifact evidence is sealed and gates are running.
    Verifying,
    /// Another bounded Run may be prepared.
    Retryable,
    /// An explicit recoverable blocker exists.
    Blocked,
    /// Deterministic evidence and a Receipt passed.
    Passed,
    /// A declared condition resolved false.
    Skipped,
    /// No recovery path remains.
    Failed,
    /// Owned activity was cancelled and reconciled.
    Cancelled,
}

impl TaskState {
    /// Whether `next` is one declared transition from this state.
    #[must_use]
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Eligible | Self::Skipped | Self::Cancelled
            ) | (
                Self::Eligible,
                Self::Reserved | Self::Blocked | Self::Cancelled
            ) | (
                Self::Reserved,
                Self::AwaitingInput | Self::Eligible | Self::Blocked | Self::Cancelled
            ) | (
                Self::AwaitingInput,
                Self::Running | Self::Blocked | Self::Cancelled
            ) | (
                Self::Running,
                Self::AwaitingApproval
                    | Self::Verifying
                    | Self::Blocked
                    | Self::Retryable
                    | Self::Failed
                    | Self::Cancelled
            ) | (
                Self::AwaitingApproval,
                Self::Running | Self::Blocked | Self::Failed | Self::Cancelled
            ) | (
                Self::Verifying,
                Self::Blocked | Self::Passed | Self::Retryable | Self::Failed | Self::Cancelled
            ) | (
                Self::Retryable | Self::Blocked,
                Self::Eligible | Self::Failed | Self::Cancelled
            )
        )
    }

    /// Whether the Task can no longer transition.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed | Self::Skipped | Self::Failed | Self::Cancelled
        )
    }
}

/// One bounded, durable state event without arbitrary payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TransitionEvent<S> {
    /// Caller-minted idempotency identity.
    pub event_id: Box<str>,
    /// Expected prior state.
    pub before: S,
    /// Committed next state.
    pub after: S,
}

/// Result of applying an idempotent event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionApplied {
    /// The state changed.
    Changed,
    /// The identical event was already committed.
    Duplicate,
}

/// A state event violated the closed transition contract.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The expected state does not match the current state.
    #[error("expected state does not match current state")]
    Stale,
    /// The state pair has no declared edge.
    #[error("state transition is not allowed")]
    Illegal,
    /// An event id was reused for different state data.
    #[error("event id was reused with conflicting state data")]
    ConflictingDuplicate,
    /// The active transition journal reached its hard bound.
    #[error("active transition journal reached its hard bound")]
    TransitionLimit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_machine_refuses_skips_and_terminal_revival() {
        assert!(MissionState::Draft.allows(MissionState::Validated));
        assert!(!MissionState::Draft.allows(MissionState::Running));
        assert!(!MissionState::Archived.allows(MissionState::Running));
    }

    #[test]
    fn task_machine_requires_reservation_and_local_input_wait() {
        assert!(TaskState::Eligible.allows(TaskState::Reserved));
        assert!(TaskState::Reserved.allows(TaskState::AwaitingInput));
        assert!(TaskState::AwaitingInput.allows(TaskState::Running));
        assert!(!TaskState::Eligible.allows(TaskState::Running));
    }
}
