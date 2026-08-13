//! Closed Mission parsing, filesystem identity, digest, DAG, and claim validation.

use std::collections::{BTreeMap, BTreeSet};

use runtrol_core::ProjectIdentity;
use runtrol_ledger::TaskId;
use runtrol_provider::AbsPath;
use sha2::{Digest as _, Sha256};

use crate::{
    CapabilitySelection, GateRegistry, InstructionRef, MAX_CAPABILITY_REFS, MAX_GATE_REFS,
    MAX_INSTRUCTION_BYTES, MAX_MISSION_BYTES, MAX_OUTPUT_ROOTS, MAX_TASK_KEY_BYTES, MISSION_SCHEMA,
    MissionSpec, ProviderSelector, TaskSpec, WorkspaceMode,
};

/// Stable structural validation failure class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingCode {
    /// TOML or closed schema decode failed.
    Schema,
    /// One numeric or collection bound failed.
    Bound,
    /// Task key was missing, malformed, or duplicated.
    TaskIdentity,
    /// Dependency was missing or cyclic.
    Dependency,
    /// Project-relative path escaped or was malformed.
    Path,
    /// Reviewed instruction bytes changed or were not UTF-8.
    Instruction,
    /// Parallel write output claims overlap.
    OutputOverlap,
    /// Exact gate is absent from the local registry.
    Gate,
    /// Exact capability ID and version are not locally active for this project.
    Capability,
    /// Provider selection is not a current runtime observation.
    Provider,
    /// A write Task did not request an isolated worktree.
    Workspace,
}

/// One typed validation finding without untrusted file or process output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionFinding {
    /// Stable failure class.
    pub code: FindingCode,
    /// Optional stable Task key.
    pub task: Option<Box<str>>,
}

/// Fully resolved Task safe for deterministic scheduling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedTask {
    /// Generated durable Task identity.
    pub id: TaskId,
    /// Stable project Task key.
    pub key: Box<str>,
    /// Dependency keys.
    pub depends_on: Vec<Box<str>>,
    /// Reviewed instruction identity.
    pub instruction: InstructionRef,
    /// Workspace collision posture.
    pub workspace_mode: WorkspaceMode,
    /// Resolved provider selection posture.
    pub provider_selector: ProviderSelector,
    /// Normalized output claims.
    pub output_roots: Vec<Box<str>>,
    /// Exact local gate references.
    pub gate_refs: Vec<Box<str>>,
    /// Exact approved capability versions.
    pub capability_versions: Vec<CapabilitySelection>,
}

/// Closed Mission plus its resolved exact identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedMission {
    /// Digest of exact Mission TOML bytes.
    pub mission_sha256: [u8; 32],
    /// Canonical project identity from Core.
    pub project: ProjectIdentity,
    /// Reviewed project Mission contract.
    pub spec: MissionSpec,
    /// Fully resolved Tasks.
    pub tasks: Vec<ValidatedTask>,
}

/// Stateless closed Mission validator.
#[derive(Clone, Copy, Debug, Default)]
pub struct MissionValidator;

impl MissionValidator {
    /// Parse and validate exact Mission bytes against current project, gate, and runtime observations.
    ///
    /// # Errors
    /// Returns all structural findings. It never guesses around one failure.
    pub fn validate(
        source: &[u8],
        project_root: &AbsPath,
        project: &ProjectIdentity,
        gates: &GateRegistry,
        runtime_ids: &[Box<str>],
        approved_capabilities: &[CapabilitySelection],
    ) -> Result<ValidatedMission, Vec<MissionFinding>> {
        let mut findings = Vec::new();
        if source.is_empty() || source.len() > MAX_MISSION_BYTES {
            return Err(vec![finding(FindingCode::Bound, None)]);
        }
        let text =
            core::str::from_utf8(source).map_err(|_| vec![finding(FindingCode::Schema, None)])?;
        let spec: MissionSpec =
            toml::from_str(text).map_err(|_| vec![finding(FindingCode::Schema, None)])?;
        if spec.schema.as_ref() != MISSION_SCHEMA
            || spec.name.is_empty()
            || spec.name.len() > 128
            || spec.project_id.is_empty()
            || spec.base_ref.is_empty()
        {
            findings.push(finding(FindingCode::Schema, None));
        }
        validate_limits(&spec, &mut findings);
        let mut keys = BTreeSet::new();
        for task in &spec.tasks {
            if !valid_task_key(&task.id) || !keys.insert(task.id.clone()) {
                findings.push(finding(FindingCode::TaskIdentity, Some(&task.id)));
            }
        }
        let graph = graph_of(&spec.tasks, &keys, &mut findings);
        detect_cycles(&graph, &mut findings);
        detect_overlaps(&spec.tasks, &graph, &mut findings);

        let mut tasks = Vec::with_capacity(spec.tasks.len());
        for task in &spec.tasks {
            if task.output_roots.is_empty()
                || task.output_roots.len() > MAX_OUTPUT_ROOTS
                || task.gate_refs.is_empty()
                || task.gate_refs.len() > MAX_GATE_REFS
            {
                findings.push(finding(FindingCode::Bound, Some(&task.id)));
            }
            if task.workspace_mode == WorkspaceMode::ReadOnlyBase
                && task.output_roots.iter().any(|root| {
                    root.as_ref() != ".runtrol/handoffs" && !root.starts_with(".runtrol/handoffs/")
                })
            {
                findings.push(finding(FindingCode::Workspace, Some(&task.id)));
            }
            if task.output_roots.iter().any(|path| !safe_relative(path)) {
                findings.push(finding(FindingCode::Path, Some(&task.id)));
            }
            for gate in &task.gate_refs {
                if gates.get(gate).is_none() {
                    findings.push(finding(FindingCode::Gate, Some(&task.id)));
                }
            }
            validate_capabilities(task, approved_capabilities, &mut findings);
            let provider_selector = resolve_provider(&task.provider_selector, runtime_ids)
                .map_err(|code| finding(code, Some(&task.id)));
            let instruction = resolve_instruction(project_root, task)
                .map_err(|code| finding(code, Some(&task.id)));
            if let (Ok(provider_selector), Ok(instruction)) = (provider_selector, instruction) {
                tasks.push(ValidatedTask {
                    id: TaskId::now(),
                    key: task.id.clone(),
                    depends_on: task.depends_on.clone(),
                    instruction,
                    workspace_mode: task.workspace_mode,
                    provider_selector,
                    output_roots: task.output_roots.clone(),
                    gate_refs: task.gate_refs.clone(),
                    capability_versions: task.capability_versions.clone(),
                });
            } else {
                if let Err(finding) = resolve_provider(&task.provider_selector, runtime_ids)
                    .map_err(|code| finding(code, Some(&task.id)))
                {
                    findings.push(finding);
                }
                if let Err(finding) = resolve_instruction(project_root, task)
                    .map_err(|code| finding(code, Some(&task.id)))
                {
                    findings.push(finding);
                }
            }
        }
        if project.worktree() != project_root {
            findings.push(finding(FindingCode::Workspace, None));
        }
        if !findings.is_empty() {
            findings.sort_by_key(|item| (item.code, item.task.clone()));
            findings.dedup();
            return Err(findings);
        }
        Ok(ValidatedMission {
            mission_sha256: Sha256::digest(source).into(),
            project: project.clone(),
            spec,
            tasks,
        })
    }
}

fn validate_capabilities(
    task: &TaskSpec,
    approved: &[CapabilitySelection],
    findings: &mut Vec<MissionFinding>,
) {
    let unique = task
        .capability_versions
        .iter()
        .collect::<BTreeSet<_>>()
        .len();
    if task.capability_versions.len() > MAX_CAPABILITY_REFS
        || unique != task.capability_versions.len()
        || task.capability_versions.iter().any(|selection| {
            !valid_task_key(&selection.capability_id)
                || parse_digest(&selection.version_sha256).is_none()
                || !approved.contains(selection)
        })
    {
        findings.push(finding(FindingCode::Capability, Some(&task.id)));
    }
}

fn validate_limits(spec: &MissionSpec, findings: &mut Vec<MissionFinding>) {
    let limits = spec.limits;
    if spec.tasks.is_empty()
        || spec.tasks.len() > runtrol_ledger::MAX_TASKS_PER_MISSION
        || !(1..=2).contains(&limits.max_parallel_tasks)
        || !(1..=8).contains(&limits.max_hot_providers)
        || !(1..=2).contains(&limits.max_runs_per_task)
        || limits.max_repair_cycles > 1
    {
        findings.push(finding(FindingCode::Bound, None));
    }
}

fn valid_task_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_TASK_KEY_BYTES
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !key.starts_with('-')
        && !key.ends_with('-')
        && !key.contains("--")
}

fn graph_of(
    tasks: &[TaskSpec],
    keys: &BTreeSet<Box<str>>,
    findings: &mut Vec<MissionFinding>,
) -> BTreeMap<Box<str>, Vec<Box<str>>> {
    let mut graph = BTreeMap::new();
    for task in tasks {
        if task
            .depends_on
            .iter()
            .any(|dependency| !keys.contains(dependency) || dependency == &task.id)
        {
            findings.push(finding(FindingCode::Dependency, Some(&task.id)));
        }
        graph.insert(task.id.clone(), task.depends_on.clone());
    }
    graph
}

fn detect_cycles(graph: &BTreeMap<Box<str>, Vec<Box<str>>>, findings: &mut Vec<MissionFinding>) {
    fn visit<'a>(
        key: &'a str,
        graph: &'a BTreeMap<Box<str>, Vec<Box<str>>>,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        if visiting.contains(key) {
            return true;
        }
        if !visited.insert(key) {
            return false;
        }
        visiting.insert(key);
        let cycle = graph.get(key).is_some_and(|dependencies| {
            dependencies
                .iter()
                .any(|dependency| visit(dependency, graph, visiting, visited))
        });
        visiting.remove(key);
        cycle
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for key in graph.keys() {
        if visit(key, graph, &mut visiting, &mut visited) {
            findings.push(finding(FindingCode::Dependency, Some(key)));
        }
    }
}

fn reaches(
    graph: &BTreeMap<Box<str>, Vec<Box<str>>>,
    from: &str,
    target: &str,
    seen: &mut BTreeSet<Box<str>>,
) -> bool {
    if from == target {
        return true;
    }
    if !seen.insert(from.into()) {
        return false;
    }
    graph.get(from).is_some_and(|dependencies| {
        dependencies
            .iter()
            .any(|dependency| reaches(graph, dependency, target, seen))
    })
}

fn detect_overlaps(
    tasks: &[TaskSpec],
    graph: &BTreeMap<Box<str>, Vec<Box<str>>>,
    findings: &mut Vec<MissionFinding>,
) {
    for (index, left) in tasks.iter().enumerate() {
        if left.workspace_mode != WorkspaceMode::IsolatedWorktree {
            continue;
        }
        for right in tasks
            .iter()
            .skip(index + 1)
            .filter(|task| task.workspace_mode == WorkspaceMode::IsolatedWorktree)
        {
            let ordered = reaches(graph, &left.id, &right.id, &mut BTreeSet::new())
                || reaches(graph, &right.id, &left.id, &mut BTreeSet::new());
            if !ordered
                && left.output_roots.iter().any(|left_root| {
                    right
                        .output_roots
                        .iter()
                        .any(|right_root| paths_overlap(left_root, right_root))
                })
            {
                findings.push(finding(FindingCode::OutputOverlap, Some(&right.id)));
            }
        }
    }
}

fn resolve_provider(
    selector: &str,
    runtime_ids: &[Box<str>],
) -> Result<ProviderSelector, FindingCode> {
    if selector == "operator_choice" {
        return Ok(ProviderSelector::OperatorChoice);
    }
    let exact = selector
        .strip_prefix("runtime:")
        .ok_or(FindingCode::Provider)?;
    if exact.is_empty() || !runtime_ids.iter().any(|runtime| runtime.as_ref() == exact) {
        return Err(FindingCode::Provider);
    }
    Ok(ProviderSelector::Exact(exact.into()))
}

fn resolve_instruction(root: &AbsPath, task: &TaskSpec) -> Result<InstructionRef, FindingCode> {
    if !safe_relative(&task.instruction_ref) {
        return Err(FindingCode::Path);
    }
    let declared = root
        .join(&task.instruction_ref)
        .map_err(|_| FindingCode::Path)?;
    let canonical = AbsPath::canonicalize(declared.as_str()).map_err(|_| FindingCode::Path)?;
    if !canonical.is_under(root) {
        return Err(FindingCode::Path);
    }
    let metadata =
        std::fs::symlink_metadata(canonical.as_std_path()).map_err(|_| FindingCode::Path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FindingCode::Path);
    }
    let bytes = std::fs::read(canonical.as_std_path()).map_err(|_| FindingCode::Instruction)?;
    if bytes.len() > MAX_INSTRUCTION_BYTES || core::str::from_utf8(&bytes).is_err() {
        return Err(FindingCode::Instruction);
    }
    let expected = parse_digest(&task.instruction_sha256).ok_or(FindingCode::Instruction)?;
    if Sha256::digest(&bytes).as_slice() != expected {
        return Err(FindingCode::Instruction);
    }
    Ok(InstructionRef {
        path: task.instruction_ref.clone(),
        sha256: expected,
    })
}

fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (slot, pair) in digest.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let Ok(pair) = core::str::from_utf8(pair) else {
            return None;
        };
        let Ok(byte) = u8::from_str_radix(pair, 16) else {
            return None;
        };
        *slot = byte;
    }
    Some(digest)
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn paths_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn finding(code: FindingCode, task: Option<&str>) -> MissionFinding {
    MissionFinding {
        code,
        task: task.map(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GateDefinition, WorkingDirectoryRule};
    use core::fmt::Write as _;
    use runtrol_security::LocalScope;

    struct Scratch {
        root: std::path::PathBuf,
        canonical: AbsPath,
    }

    impl Scratch {
        fn make() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-mission-validator-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear scratch");
            }
            std::fs::create_dir_all(root.join("instructions")).expect("create instructions");
            let canonical =
                AbsPath::canonicalize(root.to_str().expect("UTF-8 scratch")).expect("canonical");
            Self { root, canonical }
        }

        fn mission(&self, dependencies: &str, second_root: &str) -> Vec<u8> {
            let instruction = b"exact reviewed instruction\r\n";
            std::fs::write(self.root.join("instructions/task.md"), instruction)
                .expect("write instruction");
            let mut digest = String::with_capacity(64);
            for byte in Sha256::digest(instruction) {
                write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
            }
            format!(
                r#"schema = "runtrol.dev/mission/v1alpha1"
name = "fixture"
project_id = "project"
base_ref = "main"
require_clean_base = true

[limits]
max_parallel_tasks = 2
max_hot_providers = 2
max_runs_per_task = 2
max_repair_cycles = 1
stop_on_critical_failure = true

[[tasks]]
id = "first"
instruction_ref = "instructions/task.md"
instruction_sha256 = "{digest}"
workspace_mode = "isolated_worktree"
provider_selector = "operator_choice"
output_roots = ["src"]
gate_refs = ["check"]

[[tasks]]
id = "second"
depends_on = [{dependencies}]
instruction_ref = "instructions/task.md"
instruction_sha256 = "{digest}"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:fixture-runtime"
output_roots = ["{second_root}"]
gate_refs = ["check"]
"#
            )
            .into_bytes()
        }

        fn project(&self) -> ProjectIdentity {
            ProjectIdentity::discover(self.canonical.clone()).expect("project identity")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.root);
        }
    }

    fn gates() -> GateRegistry {
        let mut gates = GateRegistry::default();
        gates
            .register(
                LocalScope::GateRegister,
                GateDefinition {
                    id: "check".into(),
                    program: "cargo".into(),
                    arguments: vec!["test".into()],
                    working_directory: WorkingDirectoryRule::TaskWorktree,
                    timeout_ms: 60_000,
                    platforms: vec!["current".into()],
                },
            )
            .expect("register gate");
        gates
    }

    #[test]
    fn exact_files_dag_gates_and_runtime_observations_validate() {
        let scratch = Scratch::make();
        let mission = scratch.mission("\"first\"", "tests");
        let validated = MissionValidator::validate(
            &mission,
            &scratch.canonical,
            &scratch.project(),
            &gates(),
            &["fixture-runtime".into()],
            &[],
        )
        .expect("valid Mission");
        assert_eq!(validated.tasks.len(), 2);
        assert_eq!(
            validated
                .tasks
                .first()
                .expect("first task")
                .instruction
                .path
                .as_ref(),
            "instructions/task.md"
        );
    }

    #[test]
    fn missing_dependency_and_parallel_overlap_fail_closed() {
        let scratch = Scratch::make();
        let mission = scratch.mission("\"missing\"", "src/lib");
        let findings = MissionValidator::validate(
            &mission,
            &scratch.canonical,
            &scratch.project(),
            &gates(),
            &["fixture-runtime".into()],
            &[],
        )
        .expect_err("invalid Mission");
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == FindingCode::Dependency)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == FindingCode::OutputOverlap)
        );
    }

    #[test]
    fn changed_instruction_bytes_invalidate_review() {
        let scratch = Scratch::make();
        let mission = scratch.mission("\"first\"", "tests");
        std::fs::write(scratch.root.join("instructions/task.md"), b"changed")
            .expect("change instruction");
        let findings = MissionValidator::validate(
            &mission,
            &scratch.canonical,
            &scratch.project(),
            &gates(),
            &["fixture-runtime".into()],
            &[],
        )
        .expect_err("digest change must fail");
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == FindingCode::Instruction)
        );
    }

    #[test]
    fn capability_selection_requires_exact_local_id_and_digest() {
        let scratch = Scratch::make();
        let mission = String::from_utf8(scratch.mission("\"first\"", "tests"))
            .expect("Mission UTF-8")
            .replacen(
                "gate_refs = [\"check\"]",
                concat!(
                    "gate_refs = [\"check\"]\n",
                    "capability_versions = [{ capability_id = \"reviewed-skill\", ",
                    "version_sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\" }]"
                ),
                1,
            );
        let selection = CapabilitySelection {
            capability_id: "reviewed-skill".into(),
            version_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
        };
        let findings = MissionValidator::validate(
            mission.as_bytes(),
            &scratch.canonical,
            &scratch.project(),
            &gates(),
            &["fixture-runtime".into()],
            &[],
        )
        .expect_err("unapproved capability must fail");
        assert!(
            findings
                .iter()
                .any(|finding| finding.code == FindingCode::Capability)
        );
        MissionValidator::validate(
            mission.as_bytes(),
            &scratch.canonical,
            &scratch.project(),
            &gates(),
            &["fixture-runtime".into()],
            &[selection],
        )
        .expect("exact approved capability");
    }
}
