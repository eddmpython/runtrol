//! Canonical, content-addressed evidence Receipts.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    MAX_ARTIFACT_BYTES_PER_RUN, MAX_ARTIFACTS_PER_RUN, MAX_GATE_RUNS_PER_RUN, MissionId, ReceiptId,
    RunId, TaskId,
};

/// Opaque runtime observations bound to a Run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderObservation {
    /// Runtime-discovered provider identity.
    pub runtime_id: Box<str>,
    /// Runtime-discovered binary digest.
    pub binary_fingerprint: [u8; 32],
    /// Opaque model observation, if supplied structurally.
    pub model: Option<Box<str>>,
    /// Opaque provider-native session identity.
    pub native_session_id: Box<str>,
}

/// One sealed project Artifact reference.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArtifactEvidence {
    /// Normalized project-relative path.
    pub path: Box<str>,
    /// Exact file or manifest digest.
    pub sha256: [u8; 32],
    /// Declared bytes.
    pub size: u64,
}

/// One deterministic Gate result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GateEvidence {
    /// Stable local `GateDefinition` identity.
    pub id: Box<str>,
    /// Exact reviewed `GateDefinition` digest.
    pub definition_sha256: [u8; 32],
    /// Closed status, currently `passed` for a passing Receipt.
    pub status: Box<str>,
}

/// Complete input required to seal a passing Receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptInput {
    /// Mission identity.
    pub mission_id: MissionId,
    /// Task identity.
    pub task_id: TaskId,
    /// Run identity.
    pub run_id: RunId,
    /// Canonical project identity.
    pub project_id: Box<str>,
    /// Reviewed instruction digest.
    pub instruction_sha256: [u8; 32],
    /// Exact base identity.
    pub base_commit: Box<str>,
    /// Exact finish tree identity.
    pub finish_tree: Box<str>,
    /// Provider observations.
    pub provider_observation: ProviderObservation,
    /// Sealed Artifacts.
    pub artifacts: Vec<ArtifactEvidence>,
    /// Completed deterministic Gates.
    pub gates: Vec<GateEvidence>,
    /// Explicitly selected capability version digests.
    pub capability_versions: Vec<[u8; 32]>,
    /// Reviewed policy digest.
    pub policy_sha256: [u8; 32],
}

/// Canonical content-addressed passing evidence.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Receipt {
    /// Fixed schema identity.
    pub schema: Box<str>,
    /// Mission identity.
    pub mission_id: MissionId,
    /// Task identity.
    pub task_id: TaskId,
    /// Run identity.
    pub run_id: RunId,
    /// Canonical project identity.
    pub project_id: Box<str>,
    /// Reviewed instruction digest.
    pub instruction_sha256: [u8; 32],
    /// Exact base identity.
    pub base_commit: Box<str>,
    /// Exact finish tree identity.
    pub finish_tree: Box<str>,
    /// Provider observations.
    pub provider_observation: ProviderObservation,
    /// Sorted sealed Artifacts.
    pub artifacts: Vec<ArtifactEvidence>,
    /// Sorted completed deterministic Gates.
    pub gates: Vec<GateEvidence>,
    /// Sorted explicit capability versions.
    pub capability_versions: Vec<[u8; 32]>,
    /// Reviewed policy digest.
    pub policy_sha256: [u8; 32],
    /// Closed final outcome.
    pub outcome: Box<str>,
}

impl Receipt {
    /// Validate completeness, sort repeated evidence, and seal a passing Receipt.
    ///
    /// # Errors
    /// Returns [`ReceiptError`] when required facts are absent, over quota, or not passing.
    pub fn seal(mut input: ReceiptInput) -> Result<(ReceiptId, Self), ReceiptError> {
        if input.project_id.is_empty()
            || input.base_commit.is_empty()
            || input.finish_tree.is_empty()
            || input.provider_observation.runtime_id.is_empty()
            || input.provider_observation.native_session_id.is_empty()
        {
            return Err(ReceiptError::MissingIdentity);
        }
        if input.artifacts.is_empty() || input.gates.is_empty() {
            return Err(ReceiptError::MissingEvidence);
        }
        if input.artifacts.len() > MAX_ARTIFACTS_PER_RUN
            || input.gates.len() > MAX_GATE_RUNS_PER_RUN
        {
            return Err(ReceiptError::CountLimit);
        }
        let bytes = input
            .artifacts
            .iter()
            .try_fold(0_u64, |total, artifact| total.checked_add(artifact.size))
            .ok_or(ReceiptError::ArtifactBytes)?;
        if bytes > MAX_ARTIFACT_BYTES_PER_RUN {
            return Err(ReceiptError::ArtifactBytes);
        }
        if input
            .gates
            .iter()
            .any(|gate| gate.status.as_ref() != "passed")
        {
            return Err(ReceiptError::GateNotPassed);
        }
        input
            .artifacts
            .sort_by(|left, right| left.path.cmp(&right.path));
        input.gates.sort_by(|left, right| left.id.cmp(&right.id));
        input.capability_versions.sort_unstable();
        let receipt = Self {
            schema: "runtrol.dev/receipt/v1alpha1".into(),
            mission_id: input.mission_id,
            task_id: input.task_id,
            run_id: input.run_id,
            project_id: input.project_id,
            instruction_sha256: input.instruction_sha256,
            base_commit: input.base_commit,
            finish_tree: input.finish_tree,
            provider_observation: input.provider_observation,
            artifacts: input.artifacts,
            gates: input.gates,
            capability_versions: input.capability_versions,
            policy_sha256: input.policy_sha256,
            outcome: "passed".into(),
        };
        let canonical = receipt.canonical_bytes()?;
        Ok((
            ReceiptId::from_digest(Sha256::digest(canonical).into()),
            receipt,
        ))
    }

    /// Stable UTF-8 JSON encoding used for the Receipt identity.
    ///
    /// # Errors
    /// Returns [`ReceiptError::Encode`] if the closed structure cannot be serialized.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReceiptError> {
        serde_json::to_vec(self).map_err(|_| ReceiptError::Encode)
    }
}

/// A Receipt was incomplete or exceeded a hard evidence bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// One exact identity was absent.
    #[error("receipt is missing a required identity")]
    MissingIdentity,
    /// No Artifact or Gate evidence was supplied.
    #[error("receipt is missing required artifact or gate evidence")]
    MissingEvidence,
    /// An Artifact or Gate count exceeded its quota.
    #[error("receipt evidence count exceeded its quota")]
    CountLimit,
    /// Artifact bytes overflowed or exceeded their quota.
    #[error("receipt artifact bytes exceeded their quota")]
    ArtifactBytes,
    /// At least one required Gate did not pass.
    #[error("receipt contains a gate that did not pass")]
    GateNotPassed,
    /// The closed Receipt structure could not be encoded.
    #[error("receipt canonical encoding failed")]
    Encode,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ReceiptInput {
        ReceiptInput {
            mission_id: MissionId::now(),
            task_id: TaskId::now(),
            run_id: RunId::now(),
            project_id: "project".into(),
            instruction_sha256: [1; 32],
            base_commit: "base".into(),
            finish_tree: "tree".into(),
            provider_observation: ProviderObservation {
                runtime_id: "runtime".into(),
                binary_fingerprint: [2; 32],
                model: None,
                native_session_id: "native".into(),
            },
            artifacts: vec![ArtifactEvidence {
                path: "report.md".into(),
                sha256: [3; 32],
                size: 7,
            }],
            gates: vec![GateEvidence {
                id: "check".into(),
                definition_sha256: [4; 32],
                status: "passed".into(),
            }],
            capability_versions: Vec::new(),
            policy_sha256: [5; 32],
        }
    }

    #[test]
    fn canonical_receipt_is_stable_and_complete() {
        let (first_id, first) = Receipt::seal(input()).expect("seal");
        let (second_id, second) = Receipt::seal(ReceiptInput {
            mission_id: first.mission_id,
            task_id: first.task_id,
            run_id: first.run_id,
            ..input()
        })
        .expect("seal again");
        assert_eq!(
            first_id, second_id,
            "the same logical evidence must have one identity"
        );
        assert_eq!(first.outcome.as_ref(), "passed");
        assert_eq!(second.outcome.as_ref(), "passed");

        let (changed_id, _) = Receipt::seal(ReceiptInput {
            mission_id: first.mission_id,
            task_id: first.task_id,
            run_id: first.run_id,
            finish_tree: "changed-tree".into(),
            ..input()
        })
        .expect("seal changed evidence");
        assert_ne!(first_id, changed_id);
    }

    #[test]
    fn missing_or_failed_evidence_cannot_pass() {
        assert_eq!(
            Receipt::seal(ReceiptInput {
                artifacts: Vec::new(),
                ..input()
            }),
            Err(ReceiptError::MissingEvidence)
        );
        assert_eq!(
            Receipt::seal(ReceiptInput {
                gates: vec![GateEvidence {
                    id: "check".into(),
                    definition_sha256: [4; 32],
                    status: "failed".into()
                }],
                ..input()
            }),
            Err(ReceiptError::GateNotPassed)
        );
    }
}
