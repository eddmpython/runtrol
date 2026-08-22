//! Mission evidence identifiers and bounded records.

use core::{fmt, str::FromStr};

use runtrol_provider::SessionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    MAX_TRANSITIONS_PER_MISSION, MissionState, StateError, TaskState, TransitionApplied,
    TransitionEvent,
};

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint a time-sortable UUIDv7 identity.
            #[must_use]
            pub fn now() -> Self { Self(Uuid::now_v7()) }

            /// Raw UUID bytes for ordered storage.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 16] { self.0.as_bytes() }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0.as_hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Display::fmt(self, f) }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let raw = text.strip_prefix($prefix).ok_or(IdError)?;
                Uuid::parse_str(raw).map(Self).map_err(|_| IdError)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

/// An evidence identifier was malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("evidence identifier has the wrong prefix or UUID shape")]
pub struct IdError;

uuid_id!(
    /// One reviewed Mission graph.
    MissionId,
    "msn_"
);
uuid_id!(
    /// One reviewed future Mission start.
    ScheduleId,
    "sch_"
);
uuid_id!(
    /// One schedulable Task.
    TaskId,
    "tsk_"
);
uuid_id!(
    /// One Task execution attempt.
    RunId,
    "run_"
);
uuid_id!(
    /// One deterministic gate execution.
    GateRunId,
    "gtr_"
);

/// Content-addressed Receipt identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReceiptId([u8; 32]);

impl ReceiptId {
    /// Build from the canonical Receipt digest.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ReceiptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("rcp_")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ReceiptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for ReceiptId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex = text.strip_prefix("rcp_").ok_or(IdError)?;
        if hex.len() != 64 {
            return Err(IdError);
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let pair = core::str::from_utf8(pair).map_err(|_| IdError)?;
            let slot = bytes.get_mut(index).ok_or(IdError)?;
            *slot = u8::from_str_radix(pair, 16).map_err(|_| IdError)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for ReceiptId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ReceiptId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Content-addressed Artifact identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub [u8; 32]);

impl fmt::Debug for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("art_")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Exact Receipt authority that moved one Mission from integration to completion.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IntegrationRecord {
    /// Explicit winner for a comparison Mission, absent for an all-Task Mission.
    pub selected_task_id: Option<TaskId>,
    /// Content-addressed Receipt paired with the selected comparison Task.
    pub selected_receipt_id: Option<ReceiptId>,
}

/// Durable lifecycle of one reviewed future Mission start.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MissionScheduleState {
    /// Waiting for the exact reviewed wall-clock instant.
    Pending,
    /// Due authority was claimed and Core is preparing or submitting the first Task wave.
    Launching,
    /// The first Task wave was submitted through the existing session boundary.
    Started,
    /// The operator cancelled the future start before its due instant.
    Cancelled,
    /// Exact Mission, Gate, provider, or workspace authority no longer matched at the due instant.
    Refused,
    /// Launch began but could not finish without an explicit operator decision.
    Attention,
}

/// One exact runtime-discovered provider assignment retained without provider-specific meaning.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MissionScheduleProvider {
    /// Durable Task identity.
    pub task_id: TaskId,
    /// Opaque runtime-discovered provider identity.
    pub provider_runtime_id: Box<str>,
}

/// One bounded, durable future Mission start authority.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MissionSchedule {
    /// Caller-minted identity that makes an ambiguous schedule response idempotent.
    pub id: ScheduleId,
    /// Inclusive Unix millisecond instant when Core may claim the start.
    pub due_unix_ms: u64,
    /// Mission bytes reviewed with this schedule.
    pub mission_sha256: [u8; 32],
    /// Gate and capability policy reviewed with this schedule.
    pub policy_sha256: [u8; 32],
    /// Complete Task-to-provider map in stable Task identity order.
    pub providers: Vec<MissionScheduleProvider>,
    /// Current closed schedule state.
    pub state: MissionScheduleState,
    /// First instant Core durably claimed this schedule.
    pub claimed_unix_ms: Option<u64>,
    /// Stable structural reason code, never provider output or conversation content.
    pub failure: Option<Box<str>>,
}

/// Durable Mission row and its bounded transition journal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MissionRecord {
    /// Mission identity.
    pub id: MissionId,
    /// Digest of the closed Mission file.
    pub mission_sha256: [u8; 32],
    /// Project-owned display name.
    #[serde(default)]
    pub display_name: Box<str>,
    /// Project-relative Mission source path.
    #[serde(default)]
    pub mission_ref: Box<str>,
    /// Canonical project identity.
    pub project_id: Box<str>,
    /// Exact reviewed Mission and local Gate policy digest.
    #[serde(default)]
    pub policy_sha256: [u8; 32],
    /// Exclusive Unix millisecond deadline for the local start approval.
    #[serde(default)]
    pub approval_expires_unix_ms: u64,
    /// Exact authority that completed integration, retained through terminal compaction.
    #[serde(default)]
    pub integration: Option<IntegrationRecord>,
    /// Current reviewed future start and its terminal structural result.
    #[serde(default)]
    pub schedule: Option<MissionSchedule>,
    /// Current state.
    pub state: MissionState,
    /// Idempotent state journal, compacted only after a terminal checkpoint.
    pub transitions: Vec<TransitionEvent<MissionState>>,
}

impl MissionRecord {
    /// Create an authority-free Draft record.
    #[must_use]
    pub fn draft(mission_sha256: [u8; 32], project_id: Box<str>) -> Self {
        Self {
            id: MissionId::now(),
            mission_sha256,
            display_name: "".into(),
            mission_ref: "".into(),
            project_id,
            policy_sha256: [0; 32],
            approval_expires_unix_ms: 0,
            integration: None,
            schedule: None,
            state: MissionState::Draft,
            transitions: Vec::new(),
        }
    }

    /// Apply one legal, idempotent state event.
    ///
    /// # Errors
    /// Returns [`StateError`] for stale, illegal, conflicting, or over-limit events.
    pub fn transition(
        &mut self,
        event_id: Box<str>,
        expected: MissionState,
        next: MissionState,
    ) -> Result<TransitionApplied, StateError> {
        apply_transition(
            &mut self.state,
            &mut self.transitions,
            event_id,
            expected,
            next,
            MissionState::allows,
        )
    }

    /// Compact a terminal journal into the current snapshot.
    pub fn compact(&mut self) {
        if self.state.is_terminal() {
            self.transitions.clear();
        }
    }
}

/// Durable Task row and its bounded transition journal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TaskRecord {
    /// Task identity.
    pub id: TaskId,
    /// Owning Mission.
    pub mission_id: MissionId,
    /// Stable project Task key.
    #[serde(default)]
    pub task_key: Box<str>,
    /// Digest of reviewed instruction bytes.
    pub instruction_sha256: [u8; 32],
    /// Project-relative instruction path.
    pub instruction_ref: Box<str>,
    /// Canonical prepared worktree identity, when preparation committed.
    #[serde(default)]
    pub workspace_id: Option<Box<str>>,
    /// Exact reviewed base commit, when Git preparation committed.
    #[serde(default)]
    pub base_commit: Option<Box<str>>,
    /// Whether Runtrol created the linked worktree and therefore owns cleanup.
    #[serde(default)]
    pub workspace_owned: bool,
    /// Current state.
    pub state: TaskState,
    /// Idempotent state journal.
    pub transitions: Vec<TransitionEvent<TaskState>>,
}

impl TaskRecord {
    /// Create a Pending Task without execution authority.
    #[must_use]
    pub fn pending(
        mission_id: MissionId,
        instruction_ref: Box<str>,
        instruction_sha256: [u8; 32],
    ) -> Self {
        Self {
            id: TaskId::now(),
            mission_id,
            task_key: "".into(),
            instruction_sha256,
            instruction_ref,
            workspace_id: None,
            base_commit: None,
            workspace_owned: false,
            state: TaskState::Pending,
            transitions: Vec::new(),
        }
    }

    /// Apply one legal, idempotent state event.
    ///
    /// # Errors
    /// Returns [`StateError`] for stale, illegal, conflicting, or over-limit events.
    pub fn transition(
        &mut self,
        event_id: Box<str>,
        expected: TaskState,
        next: TaskState,
    ) -> Result<TransitionApplied, StateError> {
        apply_transition(
            &mut self.state,
            &mut self.transitions,
            event_id,
            expected,
            next,
            TaskState::allows,
        )
    }

    /// Compact a terminal journal into the current snapshot.
    pub fn compact(&mut self) {
        if self.state.is_terminal() {
            self.transitions.clear();
        }
    }
}

fn apply_transition<S: Copy + PartialEq>(
    current: &mut S,
    transitions: &mut Vec<TransitionEvent<S>>,
    event_id: Box<str>,
    expected: S,
    next: S,
    allows: impl Fn(S, S) -> bool,
) -> Result<TransitionApplied, StateError> {
    if let Some(existing) = transitions.iter().find(|event| event.event_id == event_id) {
        return if existing.before == expected && existing.after == next {
            Ok(TransitionApplied::Duplicate)
        } else {
            Err(StateError::ConflictingDuplicate)
        };
    }
    if *current != expected {
        return Err(StateError::Stale);
    }
    if !allows(expected, next) {
        return Err(StateError::Illegal);
    }
    if transitions.len() >= MAX_TRANSITIONS_PER_MISSION {
        return Err(StateError::TransitionLimit);
    }
    transitions.push(TransitionEvent {
        event_id,
        before: expected,
        after: next,
    });
    *current = next;
    Ok(TransitionApplied::Changed)
}

/// Final Run classification.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    /// Deterministic evidence passed.
    Passed,
    /// A typed failure ended the Run.
    Failed,
    /// Owned work was cancelled.
    Cancelled,
}

/// Metadata binding one Task attempt to a provider-native session and exact workspace.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunRecord {
    /// Run identity.
    pub id: RunId,
    /// Owning Task.
    pub task_id: TaskId,
    /// Bounded attempt number.
    pub attempt: u8,
    /// Existing Runtrol session identity.
    pub session_id: SessionId,
    /// Opaque provider runtime observation.
    pub provider_runtime_id: Box<str>,
    /// Exact provider binary fingerprint, filled before evidence can pass.
    pub binary_fingerprint: Option<[u8; 32]>,
    /// Canonical working-tree identity.
    pub working_tree_id: Box<str>,
    /// Reviewed instruction digest.
    pub instruction_sha256: [u8; 32],
    /// Reviewed policy digest.
    pub policy_sha256: [u8; 32],
    /// Unique local submission action, absent before local Send.
    pub submission_action_id: Option<Box<str>>,
    /// Final outcome, absent while active.
    pub outcome: Option<RunOutcome>,
}

/// Deterministic gate metadata without command or output bodies.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GateRunRecord {
    /// Gate Run identity.
    pub id: GateRunId,
    /// Owning Run.
    pub run_id: RunId,
    /// Stable local registry reference.
    pub gate_id: Box<str>,
    /// Digest of the fixed registry definition.
    pub definition_sha256: [u8; 32],
    /// Closed exit class.
    pub outcome: Box<str>,
    /// Monotonic duration in milliseconds.
    pub duration_ms: u64,
}

/// Artifact manifest metadata without Artifact bytes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArtifactRecord {
    /// Content identity.
    pub id: ArtifactId,
    /// Owning Run.
    pub run_id: RunId,
    /// Project-relative normalized path.
    pub path: Box<str>,
    /// File or manifest digest.
    pub sha256: [u8; 32],
    /// Declared bytes.
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_event_is_idempotent_but_conflicting_reuse_fails() {
        let mut mission = MissionRecord::draft([1; 32], "project".into());
        assert_eq!(
            mission.transition("event".into(), MissionState::Draft, MissionState::Validated),
            Ok(TransitionApplied::Changed)
        );
        assert_eq!(
            mission.transition("event".into(), MissionState::Draft, MissionState::Validated),
            Ok(TransitionApplied::Duplicate)
        );
        assert_eq!(
            mission.transition("event".into(), MissionState::Draft, MissionState::Rejected),
            Err(StateError::ConflictingDuplicate)
        );
    }

    #[test]
    fn mission_rows_from_before_optional_authority_remain_readable() {
        let mission = MissionRecord::draft([2; 32], "project".into());
        let mut encoded = serde_json::to_value(mission).expect("encode Mission row");
        encoded
            .as_object_mut()
            .expect("Mission row is a JSON object")
            .remove("integration");
        encoded
            .as_object_mut()
            .expect("Mission row is a JSON object")
            .remove("schedule");

        let restored: MissionRecord =
            serde_json::from_value(encoded).expect("read the prior Mission row shape");
        assert_eq!(restored.integration, None);
        assert_eq!(restored.schedule, None);
    }
}
