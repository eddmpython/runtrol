//! Bounded, metadata-only Mission evidence and recovery state.
//!
//! This crate cannot accept provider conversation events, prompts, replies, process output, environment values, or
//! command lines. Project files own instructions and artifacts. The ledger retains only identities and digests.

mod receipt;
mod state;
mod store;
mod types;

pub use receipt::{
    ArtifactEvidence, GateEvidence, ProviderObservation, Receipt, ReceiptError, ReceiptInput,
};
pub use state::{MissionState, StateError, TaskState, TransitionApplied, TransitionEvent};
pub use store::{Ledger, LedgerError, LedgerSnapshot, ListedMissions};
pub use types::{
    ArtifactId, ArtifactRecord, GateRunId, GateRunRecord, IntegrationRecord, MissionId,
    MissionRecord, ReceiptId, RunId, RunOutcome, RunRecord, TaskId, TaskRecord,
};

/// redb cache ceiling for the separate Mission ledger.
pub const CACHE_BYTES: usize = 1024 * 1024;
/// Maximum Missions retained before terminal compaction must run.
pub const MAX_MISSIONS: usize = 100;
/// Maximum Tasks in one Mission.
pub const MAX_TASKS_PER_MISSION: usize = 1_000;
/// Maximum Runs retained for one Task.
pub const MAX_RUNS_PER_TASK: usize = 2;
/// Maximum Gate Runs retained for one Run.
pub const MAX_GATE_RUNS_PER_RUN: usize = 64;
/// Maximum Artifacts retained for one Run.
pub const MAX_ARTIFACTS_PER_RUN: usize = 256;
/// Maximum combined declared Artifact bytes for one Run.
pub const MAX_ARTIFACT_BYTES_PER_RUN: u64 = 512 * 1024 * 1024;
/// Maximum transition records retained for one active Mission.
pub const MAX_TRANSITIONS_PER_MISSION: usize = 4_096;
/// Maximum Mission rows returned by one query.
pub const MAX_QUERY_MISSIONS: usize = 100;
