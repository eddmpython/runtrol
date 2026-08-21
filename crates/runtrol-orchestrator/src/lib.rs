//! Deterministic, provider-neutral Mission validation and scheduling.
//!
//! The scheduler returns typed effects. It cannot construct or submit provider input, start a driver, inspect a
//! conversation event, execute a command, or merge a working tree. Daemon composition owns those effects.

mod gate;
mod handoff;
mod scheduler;
mod spec;
mod validate;

pub use gate::{
    GateDefinition, GateError, GateOutcome, GateRegistry, GateRequest, WorkingDirectoryRule,
};
pub use handoff::{ArtifactManifest, ArtifactManifestEntry, Handoff, HandoffError};
pub use scheduler::{
    Eligibility, LocalInstructionSubmission, RecoveryTaskState, Reservation, ResourceBudget,
    Scheduler, SchedulerEffect, SchedulerError,
};
pub use spec::{
    CapabilitySelection, CompletionPolicy, InstructionRef, MissionLimits, MissionSpec,
    ProviderSelector, TaskSpec, WorkspaceMode,
};
pub use validate::{
    FindingCode, MissionFinding, MissionValidator, ValidatedMission, ValidatedTask,
};

/// Accepted Mission schema.
pub const MISSION_SCHEMA: &str = "runtrol.dev/mission/v1alpha1";
/// Maximum Mission TOML bytes.
pub const MAX_MISSION_BYTES: usize = 256 * 1024;
/// Maximum reviewed instruction bytes.
pub const MAX_INSTRUCTION_BYTES: usize = 256 * 1024;
/// Maximum stable Task key bytes.
pub const MAX_TASK_KEY_BYTES: usize = 64;
/// Maximum output roots declared by one Task.
pub const MAX_OUTPUT_ROOTS: usize = 32;
/// Maximum deterministic gates declared by one Task.
pub const MAX_GATE_REFS: usize = 64;
/// Maximum exact project capability selections declared by one Task.
pub const MAX_CAPABILITY_REFS: usize = 32;
