//! Closed Mission TOML schema and resolved provider-neutral values.

use serde::{Deserialize, Serialize};

/// Closed, versioned project Mission file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MissionSpec {
    /// Exact schema identity.
    pub schema: Box<str>,
    /// Operator-visible name from the project file.
    pub name: Box<str>,
    /// Canonical project identity selected during review.
    pub project_id: Box<str>,
    /// Explicit Git ref or non-Git base identity.
    pub base_ref: Box<str>,
    /// Whether validation refuses a changed base.
    pub require_clean_base: bool,
    /// Numeric execution bounds.
    pub limits: MissionLimits,
    /// Closed Task graph.
    pub tasks: Vec<TaskSpec>,
}

/// Hard Mission scheduler bounds.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MissionLimits {
    /// Maximum simultaneous Tasks in this Mission.
    pub max_parallel_tasks: u8,
    /// Maximum provider processes reserved across this Mission.
    pub max_hot_providers: u8,
    /// Maximum attempts for one Task.
    pub max_runs_per_task: u8,
    /// Maximum reviewed repair cycles.
    pub max_repair_cycles: u8,
    /// Whether one critical terminal failure blocks new work.
    pub stop_on_critical_failure: bool,
}

/// One schedulable Task from the reviewed Mission file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    /// Stable project-local Task key.
    pub id: Box<str>,
    /// Keys that must pass before this Task is eligible.
    #[serde(default)]
    pub depends_on: Vec<Box<str>>,
    /// Project-relative reviewed instruction file.
    pub instruction_ref: Box<str>,
    /// Lowercase SHA-256 of exact instruction bytes.
    pub instruction_sha256: Box<str>,
    /// Read-only base or isolated write worktree.
    pub workspace_mode: WorkspaceMode,
    /// Operator choice or one exact current runtime observation.
    pub provider_selector: Box<str>,
    /// Explicit project Handoffs this Task may read.
    #[serde(default)]
    pub handoff_refs: Vec<Box<str>>,
    /// Project-relative paths whose final content becomes evidence.
    pub output_roots: Vec<Box<str>>,
    /// Exact local registry IDs.
    pub gate_refs: Vec<Box<str>>,
    /// Exact approved project capabilities selected during review.
    #[serde(default)]
    pub capability_versions: Vec<CapabilitySelection>,
}

/// One explicit project capability selection with no implicit content injection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySelection {
    /// Stable project capability identity.
    pub capability_id: Box<str>,
    /// Exact lowercase full-tree digest approved locally.
    pub version_sha256: Box<str>,
}

/// Workspace collision posture.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// Observe the selected base tree without a writer claim.
    ReadOnlyBase,
    /// Use one Mission-owned linked worktree and exclusive writer claim.
    IsolatedWorktree,
}

/// Resolved provider choice without provider-name knowledge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSelector {
    /// A local operator must choose one current runtime before reservation.
    OperatorChoice,
    /// One exact opaque runtime identity observed during review.
    Exact(Box<str>),
}

/// Project-owned reviewed instruction identity, never its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionRef {
    /// Project-relative path.
    pub path: Box<str>,
    /// SHA-256 of exact UTF-8 bytes.
    pub sha256: [u8; 32],
}
