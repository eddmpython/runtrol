//! Provider-neutral local Mission control above the scheduler and public Runtime session boundary.

mod scheduling;

pub(crate) use scheduling::MissionScheduleExecution;
use scheduling::schedule_line;

use std::{
    collections::BTreeMap,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use runtrol_childproc::{Containment, SpawnError, capture, capture_in, resolve};
use runtrol_core::ProjectIdentity;
use runtrol_ipc::wire::{
    GateLine, MissionArtifactLine, MissionCapabilityLine, MissionInstruction,
    MissionIntegrationLine, MissionLine, MissionScheduleLine, MissionScheduleProviderLine,
    MissionSnapshot, MissionTaskLine, MissionWorkspace, Request, Response,
};
use runtrol_ledger::{
    ArtifactEvidence, ArtifactId, ArtifactRecord, GateEvidence, GateRunRecord, IntegrationRecord,
    Ledger, LedgerSnapshot, MissionId, MissionRecord, MissionSchedule, MissionScheduleProvider,
    MissionScheduleState, MissionState, ProviderObservation, Receipt, ReceiptId, ReceiptInput,
    RunOutcome, RunRecord, ScheduleId, TaskId, TaskRecord, TaskState,
};
use runtrol_orchestrator::{
    CapabilitySelection, CompletionPolicy, GateDefinition, GateOutcome, GateRegistry, GateRequest,
    MAX_INSTRUCTION_BYTES, MAX_MISSION_BYTES, MissionValidator, ProviderSelector,
    RecoveryTaskState, ResourceBudget, Scheduler, SchedulerEffect, SchedulerError,
    ValidatedMission, WorkingDirectoryRule, WorkspaceMode,
};
use runtrol_provider::AbsPath;
use runtrol_security::LocalScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

type MissionFlightBinding = (Box<str>, [u8; 32]);

/// Local Mission state not duplicated in the ledger: resolved scheduler values and live public session bindings.
#[derive(Clone, Debug)]
struct ActiveMission {
    validated: ValidatedMission,
    scheduler: Option<Scheduler>,
    workspaces: BTreeMap<runtrol_ledger::TaskId, WorkspaceBinding>,
    sessions: BTreeMap<runtrol_ledger::TaskId, SessionBinding>,
}

#[derive(Clone, Debug)]
struct SessionBinding {
    runtime_session: Box<str>,
    provider_runtime: Box<str>,
    native_session: Option<Box<str>>,
}

#[derive(Clone, Debug)]
struct WorkspaceBinding {
    workspace: AbsPath,
    base_commit: Box<str>,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceIntent {
    mission_id: MissionId,
    task_id: runtrol_ledger::TaskId,
    base_worktree: AbsPath,
    common_store: AbsPath,
    target: AbsPath,
    base_ref: Box<str>,
    require_clean_base: bool,
    isolated: bool,
}

pub(crate) enum WorkspacePreparation {
    Ready(Response),
    Run(WorkspaceIntent),
}

#[derive(Clone, Debug)]
pub(crate) struct VerificationIntent {
    mission_id: MissionId,
    task_id: runtrol_ledger::TaskId,
    run_id: runtrol_ledger::RunId,
    workspace: AbsPath,
    base_commit: Box<str>,
    project_id: Box<str>,
    provider_runtime_id: Box<str>,
    native_session_id: Box<str>,
    instruction_sha256: [u8; 32],
    policy_sha256: [u8; 32],
    output_roots: Vec<Box<str>>,
    gate_requests: Vec<GateRequest>,
    capability_versions: Vec<[u8; 32]>,
}

#[derive(Debug)]
struct GateResult {
    request: GateRequest,
    outcome: GateOutcome,
    duration_ms: u64,
}

#[derive(Debug)]
struct VerificationEvidence {
    binary_fingerprint: [u8; 32],
    artifacts: Vec<ArtifactEvidence>,
    finish_tree: Box<str>,
    gates: Vec<GateResult>,
}

#[derive(Clone, Debug)]
struct IntegrationIntent {
    mission_id: MissionId,
    project: AbsPath,
    selected_task_id: Option<TaskId>,
    selected_receipt_id: Option<ReceiptId>,
    expected_artifacts: Vec<ArtifactEvidence>,
    gate_requests: Vec<GateRequest>,
}

#[derive(Debug)]
struct IntegrationEvidence {
    artifacts: Vec<ArtifactEvidence>,
    gates: Vec<GateResult>,
}

/// Local controller. The ledger remains the durable Mission state SSOT.
#[derive(Debug, Default)]
pub(crate) struct MissionController {
    gates: GateRegistry,
    active: BTreeMap<MissionId, ActiveMission>,
    gate_path: Option<AbsPath>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GateFile {
    schema: u8,
    definitions: Vec<GateDefinition>,
}

const GATE_FILE_SCHEMA: u8 = 1;
const MAX_GATE_FILE_BYTES: u64 = 256 * 1024;
const MISSION_START_APPROVAL_WINDOW: Duration = Duration::from_mins(5);

impl MissionController {
    pub(crate) fn open(gate_path: AbsPath) -> Result<Self, String> {
        Ok(Self {
            gates: load_gates(&gate_path)?,
            active: BTreeMap::new(),
            gate_path: Some(gate_path),
        })
    }

    pub(crate) fn recover(
        &mut self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        growth: &mut crate::growth::GrowthController,
    ) -> Result<(), String> {
        let listed = ledger
            .list(runtrol_ledger::MAX_QUERY_MISSIONS)
            .map_err(|error| error.to_string())?;
        if listed.truncated {
            return Err("the Mission recovery query exceeded its fixed bound".to_owned());
        }
        for mut snapshot in listed.missions {
            if snapshot.mission.state.is_terminal() {
                continue;
            }
            let Ok(mut validated) = self.revalidate_snapshot(&snapshot, runtime_ids, growth) else {
                block_unrecoverable(ledger, &mut snapshot)?;
                continue;
            };
            if bind_durable_task_ids(&mut validated, &snapshot).is_err() {
                block_unrecoverable(ledger, &mut snapshot)?;
                continue;
            }
            let Ok(workspaces) = recover_workspaces(&validated, &snapshot) else {
                block_unrecoverable(ledger, &mut snapshot)?;
                continue;
            };
            let mut unsafe_recovery = false;
            let mut recovered = BTreeMap::new();
            for task in &mut snapshot.tasks {
                let state = match task.state {
                    TaskState::Passed => RecoveryTaskState::Passed,
                    TaskState::Pending | TaskState::Eligible => RecoveryTaskState::Open,
                    TaskState::Reserved => {
                        task.transition(
                            format!("restart-release-{}", task.id).into(),
                            TaskState::Reserved,
                            TaskState::Eligible,
                        )
                        .map_err(|_| "a reserved Task could not be released during recovery")?;
                        RecoveryTaskState::Open
                    }
                    TaskState::AwaitingInput
                    | TaskState::Running
                    | TaskState::AwaitingApproval
                    | TaskState::Verifying => {
                        task.transition(
                            format!("restart-block-{}", task.id).into(),
                            task.state,
                            TaskState::Blocked,
                        )
                        .map_err(|_| "an ambiguous Task could not be blocked during recovery")?;
                        unsafe_recovery = true;
                        RecoveryTaskState::Terminal
                    }
                    TaskState::Retryable | TaskState::Blocked => RecoveryTaskState::Terminal,
                    TaskState::Skipped | TaskState::Failed | TaskState::Cancelled => {
                        RecoveryTaskState::Terminal
                    }
                };
                recovered.insert(task.id, state);
            }
            if unsafe_recovery && snapshot.mission.state == MissionState::Running {
                snapshot
                    .mission
                    .transition(
                        "restart-ambiguous".into(),
                        MissionState::Running,
                        MissionState::Blocked,
                    )
                    .map_err(|_| "the Mission could not enter its recovery blocker")?;
            }
            let mut scheduler = if matches!(
                snapshot.mission.state,
                MissionState::Running | MissionState::Paused | MissionState::Blocked
            ) {
                Some(
                    Scheduler::recover(
                        validated.clone(),
                        ResourceBudget {
                            max_hot_providers: validated.spec.limits.max_hot_providers,
                        },
                        &recovered,
                        snapshot.mission.state != MissionState::Running,
                    )
                    .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            reserve_recovered_work(&mut snapshot, scheduler.as_mut())?;
            ledger.put(&snapshot).map_err(|error| error.to_string())?;
            self.active.insert(
                snapshot.mission.id,
                ActiveMission {
                    validated,
                    scheduler,
                    workspaces,
                    sessions: BTreeMap::new(),
                },
            );
        }
        Ok(())
    }

    fn revalidate_snapshot(
        &self,
        snapshot: &LedgerSnapshot,
        runtime_ids: &[Box<str>],
        growth: &mut crate::growth::GrowthController,
    ) -> Result<ValidatedMission, String> {
        if !safe_relative(&snapshot.mission.mission_ref) {
            return Err("a recovered Mission source path is invalid".to_owned());
        }
        let project = AbsPath::canonicalize(&snapshot.mission.project_id)
            .map_err(|_| "a recovered Mission project is unavailable".to_owned())?;
        let identity = ProjectIdentity::discover(project.clone())
            .map_err(|_| "a recovered Mission project identity is unavailable".to_owned())?;
        let source_path = project
            .join(&snapshot.mission.mission_ref)
            .map_err(|_| "a recovered Mission source path is invalid".to_owned())?;
        let canonical = AbsPath::canonicalize(source_path.as_str())
            .map_err(|_| "a recovered Mission source is unavailable".to_owned())?;
        if !canonical.is_under(&project) {
            return Err("a recovered Mission source escaped its project".to_owned());
        }
        let source = std::fs::read(canonical.as_std_path())
            .map_err(|_| "a recovered Mission source cannot be read".to_owned())?;
        if Sha256::digest(&source).as_slice() != snapshot.mission.mission_sha256 {
            return Err("a recovered Mission source digest changed".to_owned());
        }
        let approved_capabilities = growth.approved_capabilities(project.as_str())?;
        let validated = MissionValidator::validate(
            &source,
            &project,
            &identity,
            &self.gates,
            runtime_ids,
            &approved_capabilities,
        )
        .map_err(|_| "a recovered Mission no longer validates".to_owned())?;
        if policy_digest(&self.gates, &validated) != snapshot.mission.policy_sha256 {
            return Err("a recovered Mission policy digest changed".to_owned());
        }
        Ok(validated)
    }

    /// Answer one scope-authorized Mission request.
    pub(crate) fn answer(
        &mut self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        approved_capabilities: &[CapabilitySelection],
        request: &Request,
    ) -> Response {
        self.try_answer(ledger, runtime_ids, approved_capabilities, request)
            .unwrap_or_else(failed)
    }

    fn try_answer(
        &mut self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        approved_capabilities: &[CapabilitySelection],
        request: &Request,
    ) -> Result<Response, &'static str> {
        if let Some(answer) = self.try_schedule_answer(ledger, runtime_ids, request) {
            return answer;
        }
        match request {
            Request::MissionRegisterGate {
                gate_id,
                program,
                arguments,
                timeout_ms,
            } => {
                let previous = self.gates.clone();
                self.gates
                    .register(
                        LocalScope::GateRegister,
                        GateDefinition {
                            id: gate_id.clone(),
                            program: program.clone(),
                            arguments: arguments.clone(),
                            working_directory: WorkingDirectoryRule::TaskWorktree,
                            timeout_ms: *timeout_ms,
                            platforms: vec![std::env::consts::OS.into()],
                        },
                    )
                    .map_err(|_| "the local gate definition is invalid")?;
                if self.save_gates().is_err() {
                    self.gates = previous;
                    return Err("the local gate registry could not be committed");
                }
                Ok(Response::Done)
            }
            Request::MissionValidate {
                project,
                mission_ref,
            } => self.validate(
                ledger,
                runtime_ids,
                approved_capabilities,
                project,
                mission_ref,
            ),
            Request::MissionListGates => Ok(Response::MissionGates(
                self.gates
                    .definitions()
                    .map(|definition| GateLine {
                        gate_id: definition.id.clone(),
                        program: definition.program.clone(),
                        timeout_ms: definition.timeout_ms,
                    })
                    .collect(),
            )),
            Request::MissionList => self.list(ledger),
            Request::MissionGet { mission_id } => self.get(ledger, mission_id),
            Request::MissionStart {
                mission_id,
                mission_sha256,
            } => self.start(ledger, approved_capabilities, mission_id, mission_sha256),
            Request::MissionPause { mission_id } => self.pause(ledger, mission_id),
            Request::MissionResumeSafe { mission_id } => self.resume_safe(ledger, mission_id),
            Request::MissionCancel { mission_id } => self.cancel(ledger, mission_id),
            Request::MissionBindSession {
                mission_id,
                task_id,
                session_id,
                provider_runtime_id,
                native_session_id,
                workspace,
            } => self.bind_session(
                ledger,
                mission_id,
                task_id,
                session_id,
                provider_runtime_id,
                native_session_id.as_deref(),
                workspace,
            ),
            Request::MissionSendTaskInstruction {
                mission_id,
                task_id,
                instruction_sha256,
            } => self.send_instruction(
                ledger,
                approved_capabilities,
                mission_id,
                task_id,
                instruction_sha256,
            ),
            Request::MissionRetryTask {
                mission_id,
                task_id,
            } => self.retry_task(ledger, mission_id, task_id),
            Request::MissionArchive { mission_id } => self.archive(ledger, mission_id),
            _ => Err("the request is not a Mission operation"),
        }
    }

    fn try_schedule_answer(
        &self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        request: &Request,
    ) -> Option<Result<Response, &'static str>> {
        match request {
            Request::MissionSchedule {
                schedule_id,
                replaces_schedule_id,
                mission_id,
                mission_sha256,
                due_unix_ms,
                providers,
            } => Some(self.schedule(
                ledger,
                runtime_ids,
                schedule_id,
                replaces_schedule_id.as_deref(),
                mission_id,
                mission_sha256,
                *due_unix_ms,
                providers,
            )),
            Request::MissionScheduleCancel {
                mission_id,
                mission_sha256,
                schedule_id,
            } => Some(self.cancel_schedule(ledger, mission_id, mission_sha256, schedule_id)),
            _ => None,
        }
    }

    fn save_gates(&self) -> Result<(), String> {
        let Some(path) = &self.gate_path else {
            return Ok(());
        };
        save_gates(path, &self.gates)
    }

    pub(crate) fn gate_requests(
        &self,
        gate_ids: &[Box<str>],
        run_id: runtrol_ledger::RunId,
    ) -> Result<Vec<GateRequest>, &'static str> {
        gate_ids
            .iter()
            .map(|gate| {
                self.gates
                    .request(gate, run_id)
                    .map_err(|_| "a capability verification Gate is not locally registered")
            })
            .collect()
    }

    fn validate(
        &mut self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        approved_capabilities: &[CapabilitySelection],
        project: &str,
        mission_ref: &str,
    ) -> Result<Response, &'static str> {
        if !safe_relative(mission_ref) {
            return Err("the Mission path must be project relative");
        }
        let project_root =
            AbsPath::canonicalize(project).map_err(|_| "the project path is unavailable")?;
        let project_identity = ProjectIdentity::discover(project_root.clone())
            .map_err(|_| "the project identity is unavailable")?;
        if project_identity.worktree() != &project_root {
            return Err("Mission validation requires the exact project working-tree root");
        }
        let mission_path = project_root
            .join(mission_ref)
            .map_err(|_| "the Mission path is invalid")?;
        let canonical = AbsPath::canonicalize(mission_path.as_str())
            .map_err(|_| "the Mission file is unavailable")?;
        if !canonical.is_under(&project_root) {
            return Err("the Mission file escapes the project");
        }
        let metadata = std::fs::symlink_metadata(canonical.as_std_path())
            .map_err(|_| "the Mission file is unavailable")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("the Mission file must be a regular project file");
        }
        let source = std::fs::read(canonical.as_std_path())
            .map_err(|_| "the Mission file cannot be read")?;
        let validated = MissionValidator::validate(
            &source,
            &project_root,
            &project_identity,
            &self.gates,
            runtime_ids,
            approved_capabilities,
        )
        .map_err(|_| "the Mission contract did not validate")?;

        let mut record =
            MissionRecord::draft(validated.mission_sha256, project_root.as_str().into());
        record.display_name = validated.spec.name.clone();
        record.mission_ref = mission_ref.into();
        record.policy_sha256 = policy_digest(&self.gates, &validated);
        record.approval_expires_unix_ms = approval_deadline()?;
        record
            .transition(
                "validation".into(),
                MissionState::Draft,
                MissionState::Validated,
            )
            .map_err(|_| "the Mission state could not be validated")?;
        let mission_id = record.id;
        let tasks = validated
            .tasks
            .iter()
            .map(|task| {
                let mut record = TaskRecord::pending(
                    mission_id,
                    task.instruction.path.clone(),
                    task.instruction.sha256,
                );
                record.id = task.id;
                record.task_key = task.key.clone();
                record
            })
            .collect();
        let snapshot = LedgerSnapshot {
            mission: record,
            tasks,
            runs: Vec::new(),
            gate_runs: Vec::new(),
            artifacts: Vec::new(),
            receipts: Vec::new(),
            compacted: false,
        };
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission ledger refused the validated Mission")?;
        self.active.insert(
            mission_id,
            ActiveMission {
                validated,
                scheduler: None,
                workspaces: BTreeMap::new(),
                sessions: BTreeMap::new(),
            },
        );
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&mission_id),
        ))
    }

    fn list(&self, ledger: &Ledger) -> Result<Response, &'static str> {
        let listed = ledger
            .list(runtrol_ledger::MAX_QUERY_MISSIONS)
            .map_err(|_| "the Mission ledger cannot be read")?;
        Ok(Response::Missions(
            listed
                .missions
                .iter()
                .map(|snapshot| line_of(snapshot, self.active.get(&snapshot.mission.id)))
                .collect(),
        ))
    }

    fn get(&self, ledger: &Ledger, mission_id: &str) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    /// Read one exact active-aware snapshot for structural Mission Flight Signal validation.
    pub(crate) fn flight_signal_snapshot(
        &self,
        ledger: &Ledger,
        mission_id: &str,
    ) -> Result<Option<MissionSnapshot>, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?;
        Ok(snapshot.map(|snapshot| snapshot_of(&snapshot, self.active.get(&id))))
    }

    /// Resolve the one active Mission that owns an exact public Runtime session.
    pub(crate) fn flight_signal_mission_for_session(
        &self,
        ledger: &Ledger,
        session_id: &str,
    ) -> Result<Option<MissionFlightBinding>, &'static str> {
        let mut found = None;
        for (mission_id, active) in &self.active {
            if !active
                .sessions
                .values()
                .any(|binding| binding.runtime_session.as_ref() == session_id)
            {
                continue;
            }
            if found.is_some() {
                return Err("more than one Mission owns the same public Runtime session");
            }
            let snapshot = ledger
                .snapshot(*mission_id)
                .map_err(|_| "the Mission ledger cannot be read")?
                .ok_or("the Mission disappeared while resolving its Runtime session")?;
            found = Some((
                mission_id.to_string().into(),
                snapshot.mission.mission_sha256,
            ));
        }
        Ok(found)
    }

    fn start(
        &mut self,
        ledger: &Ledger,
        approved_capabilities: &[CapabilitySelection],
        mission_id: &str,
        mission_sha256: &str,
    ) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let snapshot =
            self.start_snapshot(ledger, approved_capabilities, id, mission_sha256, true)?;
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission start could not be committed")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    fn start_snapshot(
        &mut self,
        ledger: &Ledger,
        approved_capabilities: &[CapabilitySelection],
        id: MissionId,
        mission_sha256: &str,
        require_fresh_approval: bool,
    ) -> Result<LedgerSnapshot, &'static str> {
        let active = self
            .active
            .get_mut(&id)
            .ok_or("the Mission must be revalidated after restart")?;
        if hex(&active.validated.mission_sha256) != mission_sha256 {
            return Err("the reviewed Mission digest changed");
        }
        let mut snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if require_fresh_approval && current_unix_ms()? >= snapshot.mission.approval_expires_unix_ms
        {
            return Err("the local Mission start approval expired; validate it again");
        }
        ensure_review_current(
            &active.validated,
            &snapshot.mission.mission_ref,
            approved_capabilities,
        )?;
        if policy_digest(&self.gates, &active.validated) != snapshot.mission.policy_sha256 {
            return Err("the reviewed Mission Gate policy changed");
        }
        snapshot
            .mission
            .transition(
                "review-ready".into(),
                MissionState::Validated,
                MissionState::Ready,
            )
            .map_err(|_| "the Mission is not validated")?;
        snapshot
            .mission
            .transition(
                "start-running".into(),
                MissionState::Ready,
                MissionState::Running,
            )
            .map_err(|_| "the Mission cannot start")?;
        let mut scheduler = Scheduler::new(
            active.validated.clone(),
            ResourceBudget {
                max_hot_providers: active.validated.spec.limits.max_hot_providers,
            },
        )
        .map_err(|_| "the Mission scheduler budget is invalid")?;
        for task in &active.validated.tasks {
            if task.depends_on.is_empty() {
                transition_task(
                    &mut snapshot,
                    task.id,
                    TaskState::Pending,
                    TaskState::Eligible,
                    "eligible",
                )?;
            }
        }
        reserve_available(&mut snapshot, &mut scheduler)?;
        active.scheduler = Some(scheduler);
        Ok(snapshot)
    }

    fn pause(&mut self, ledger: &Ledger, mission_id: &str) -> Result<Response, &'static str> {
        self.mission_transition(
            ledger,
            mission_id,
            MissionState::Running,
            MissionState::Paused,
            "pause",
            Scheduler::pause,
        )
    }

    fn resume_safe(&mut self, ledger: &Ledger, mission_id: &str) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let active = self
            .active
            .get_mut(&id)
            .ok_or("the Mission must be revalidated after restart")?;
        let scheduler = active
            .scheduler
            .as_mut()
            .ok_or("the Mission scheduler is unavailable")?;
        let mut snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let before = snapshot.mission.state;
        if !matches!(before, MissionState::Paused | MissionState::Blocked) {
            return Err("the Mission is not paused or recovery-blocked");
        }
        snapshot
            .mission
            .transition("resume-safe".into(), before, MissionState::Running)
            .map_err(|_| "the Mission cannot resume safely")?;
        scheduler.resume_safe();
        reserve_available(&mut snapshot, scheduler)?;
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission resume could not be committed")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    fn cancel(&mut self, ledger: &Ledger, mission_id: &str) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let active = self
            .active
            .get_mut(&id)
            .ok_or("the Mission must be revalidated after restart")?;
        let mut snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        snapshot
            .mission
            .transition(
                "cancel".into(),
                snapshot.mission.state,
                MissionState::Cancelled,
            )
            .map_err(|_| "the Mission cannot be cancelled from its current state")?;
        if let Some(scheduler) = &mut active.scheduler {
            let _effects = scheduler.cancel();
        }
        for task in &mut snapshot.tasks {
            if !task.state.is_terminal() && task.state.allows(TaskState::Cancelled) {
                task.transition("cancel".into(), task.state, TaskState::Cancelled)
                    .map_err(|_| "a Task could not be cancelled")?;
            }
        }
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission cancellation could not be committed")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    fn archive(&mut self, ledger: &Ledger, mission_id: &str) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let mut snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let before = snapshot.mission.state;
        if !matches!(
            before,
            MissionState::Completed | MissionState::Failed | MissionState::Cancelled
        ) {
            return Err("only a completed, failed, or cancelled Mission can be archived");
        }
        snapshot
            .mission
            .transition("archive".into(), before, MissionState::Archived)
            .map_err(|_| "the Mission cannot be archived")?;
        snapshot.compact();
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission archive could not be committed")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    fn mission_transition(
        &mut self,
        ledger: &Ledger,
        mission_id: &str,
        before: MissionState,
        after: MissionState,
        event: &'static str,
        update: impl FnOnce(&mut Scheduler),
    ) -> Result<Response, &'static str> {
        let id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let active = self
            .active
            .get_mut(&id)
            .ok_or("the Mission must be revalidated after restart")?;
        let scheduler = active
            .scheduler
            .as_mut()
            .ok_or("the Mission scheduler is unavailable")?;
        let mut snapshot = ledger
            .snapshot(id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        snapshot
            .mission
            .transition(event.into(), before, after)
            .map_err(|_| "the Mission state transition is not allowed")?;
        update(scheduler);
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission state could not be committed")?;
        Ok(Self::snapshot_response(&snapshot, self.active.get(&id)))
    }

    pub(crate) fn workspace_intent(
        &self,
        ledger: &Ledger,
        mission_id: &str,
        task_id: &str,
    ) -> Result<WorkspacePreparation, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        let active = self
            .active
            .get(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if snapshot.mission.state != MissionState::Running {
            return Err("the Mission is not running");
        }
        let record = snapshot
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("the Task does not exist")?;
        if record.state != TaskState::Reserved {
            return Err("the Task has no active workspace reservation");
        }
        if let Some(prepared) = active.workspaces.get(&task_id) {
            return Ok(WorkspacePreparation::Ready(workspace_response(
                mission_id, task_id, prepared,
            )));
        }
        let task = active
            .validated
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("the Task contract is unavailable")?;
        let base_worktree = active.validated.project.worktree().clone();
        let isolated = task.workspace_mode == WorkspaceMode::IsolatedWorktree;
        let target = if isolated {
            base_worktree
                .parent()
                .ok_or("the project has no parent for Mission worktrees")?
                .join(".runtrol-worktrees")
                .and_then(|path| path.join(&mission_id.to_string()))
                .and_then(|path| path.join(&task_id.to_string()))
                .map_err(|_| "the Mission worktree path is invalid")?
        } else {
            base_worktree.clone()
        };
        Ok(WorkspacePreparation::Run(WorkspaceIntent {
            mission_id,
            task_id,
            base_worktree,
            common_store: active.validated.project.common_store().clone(),
            target,
            base_ref: active.validated.spec.base_ref.clone(),
            require_clean_base: active.validated.spec.require_clean_base,
            isolated,
        }))
    }

    fn commit_workspace(
        &mut self,
        ledger: &Ledger,
        intent: &WorkspaceIntent,
        workspace: AbsPath,
        base_commit: Box<str>,
    ) -> Result<Response, &'static str> {
        let active = self
            .active
            .get_mut(&intent.mission_id)
            .ok_or("the Mission changed while its workspace was prepared")?;
        let mut snapshot = ledger
            .snapshot(intent.mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.id == intent.task_id)
            .ok_or("the Task does not exist")?;
        if snapshot.mission.state != MissionState::Running || task.state != TaskState::Reserved {
            return Err("the Task reservation changed while its workspace was prepared");
        }
        let binding = WorkspaceBinding {
            workspace,
            base_commit,
        };
        let record = snapshot
            .tasks
            .iter_mut()
            .find(|task| task.id == intent.task_id)
            .ok_or("the durable Task is unavailable")?;
        record.workspace_id = Some(binding.workspace.as_str().into());
        record.base_commit = Some(binding.base_commit.clone());
        record.workspace_owned = intent.isolated;
        ledger
            .put(&snapshot)
            .map_err(|_| "the Task workspace identity could not be committed")?;
        active.workspaces.insert(intent.task_id, binding.clone());
        Ok(workspace_response(
            intent.mission_id,
            intent.task_id,
            &binding,
        ))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the private wire binds every exact public Runtime session observation"
    )]
    fn bind_session(
        &mut self,
        ledger: &Ledger,
        mission_id: &str,
        task_id: &str,
        session_id: &str,
        provider_runtime_id: &str,
        native_session_id: Option<&str>,
        workspace: &str,
    ) -> Result<Response, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        if session_id.is_empty() || provider_runtime_id.is_empty() {
            return Err("the public Runtime session observation is incomplete");
        }
        let active = self
            .active
            .get_mut(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let task = active
            .validated
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("the Task does not exist")?;
        let prepared = active
            .workspaces
            .get(&task_id)
            .ok_or("the Task workspace has not been prepared")?;
        let workspace =
            AbsPath::canonicalize(workspace).map_err(|_| "the Task workspace is unavailable")?;
        if workspace != prepared.workspace {
            return Err("the public Runtime session uses a different Task workspace");
        }
        let identity = ProjectIdentity::discover(workspace.clone())
            .map_err(|_| "the Task workspace identity is unavailable")?;
        match task.workspace_mode {
            WorkspaceMode::ReadOnlyBase
                if identity.worktree() != active.validated.project.worktree() =>
            {
                return Err("a read-only Task must use the reviewed base worktree");
            }
            WorkspaceMode::IsolatedWorktree
                if identity.worktree() == active.validated.project.worktree()
                    || identity.common_store() != active.validated.project.common_store() =>
            {
                return Err(
                    "a write Task must use a distinct linked worktree in the reviewed project",
                );
            }
            WorkspaceMode::ReadOnlyBase | WorkspaceMode::IsolatedWorktree => {}
        }
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        transition_task(
            &mut snapshot,
            task_id,
            TaskState::Reserved,
            TaskState::AwaitingInput,
            format!("session-ready-{session_id}"),
        )?;
        active.sessions.insert(
            task_id,
            SessionBinding {
                runtime_session: session_id.into(),
                provider_runtime: provider_runtime_id.into(),
                native_session: native_session_id.map(Into::into),
            },
        );
        ledger
            .put(&snapshot)
            .map_err(|_| "the Task session binding could not be committed")?;
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&mission_id),
        ))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one durable local Send transaction binds all reviewed identities before transport"
    )]
    fn send_instruction(
        &mut self,
        ledger: &Ledger,
        approved_capabilities: &[CapabilitySelection],
        mission_id: &str,
        task_id: &str,
        instruction_sha256: &str,
    ) -> Result<Response, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        let active = self
            .active
            .get(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let task = active
            .validated
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("the Task does not exist")?;
        let binding = active
            .sessions
            .get(&task_id)
            .ok_or("the Task has no prepared public Runtime session")?
            .clone();
        let workspace = active
            .workspaces
            .get(&task_id)
            .ok_or("the Task workspace evidence is unavailable")?
            .clone();
        let submission = active
            .scheduler
            .as_ref()
            .ok_or("the Mission scheduler is unavailable")?
            .local_submission(task_id)
            .map_err(|_| "the Task has no active scheduler reservation")?;
        if hex(&task.instruction.sha256) != instruction_sha256 {
            return Err("the reviewed Task instruction digest changed");
        }
        let path = active
            .validated
            .project
            .worktree()
            .join(&task.instruction.path)
            .map_err(|_| "the instruction path is invalid")?;
        let canonical = AbsPath::canonicalize(path.as_str())
            .map_err(|_| "the instruction file is unavailable")?;
        if !canonical.is_under(active.validated.project.worktree()) {
            return Err("the instruction file escapes the project");
        }
        let bytes = std::fs::read(canonical.as_std_path())
            .map_err(|_| "the instruction file cannot be read")?;
        if Sha256::digest(&bytes).as_slice() != task.instruction.sha256 {
            return Err("the Task instruction bytes changed after review");
        }
        let instruction =
            String::from_utf8(bytes).map_err(|_| "the Task instruction is not UTF-8")?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        ensure_review_current(
            &active.validated,
            &snapshot.mission.mission_ref,
            approved_capabilities,
        )?;
        if policy_digest(&self.gates, &active.validated) != snapshot.mission.policy_sha256 {
            return Err("the reviewed Mission Gate policy changed");
        }
        if snapshot.runs.iter().any(|run| run.id == submission.run_id) {
            return Err("the Task instruction already has a durable submission intent");
        }
        transition_task(
            &mut snapshot,
            task_id,
            TaskState::AwaitingInput,
            TaskState::Running,
            format!("local-send-{}", submission.run_id),
        )?;
        let session_id = binding
            .runtime_session
            .parse()
            .map_err(|_| "the public Runtime session identity is invalid")?;
        let attempt = u8::try_from(
            snapshot
                .runs
                .iter()
                .filter(|run| run.task_id == task_id)
                .count()
                + 1,
        )
        .map_err(|_| "the Task attempt count is exhausted")?;
        snapshot.runs.push(RunRecord {
            id: submission.run_id,
            task_id,
            attempt,
            session_id,
            provider_runtime_id: binding.provider_runtime.clone(),
            binary_fingerprint: None,
            working_tree_id: workspace.workspace.as_str().into(),
            instruction_sha256: task.instruction.sha256,
            policy_sha256: snapshot.mission.policy_sha256,
            submission_action_id: Some(format!("submit-{}", submission.run_id).into()),
            outcome: None,
        });
        ledger
            .put(&snapshot)
            .map_err(|_| "the local Task submission intent could not be committed")?;
        Ok(Response::MissionInstruction(Box::new(MissionInstruction {
            mission_id: mission_id.to_string().into(),
            task_id: task_id.to_string().into(),
            session_id: binding.runtime_session,
            instruction: instruction.into(),
            instruction_sha256: instruction_sha256.into(),
        })))
    }

    pub(crate) fn verification_intent(
        &mut self,
        ledger: &Ledger,
        mission_id: &str,
        task_id: &str,
        native_session_id: &str,
    ) -> Result<VerificationIntent, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        let active = self
            .active
            .get_mut(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let task = active
            .validated
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("the Task does not exist")?;
        let binding = active
            .sessions
            .get_mut(&task_id)
            .ok_or("the Task has no public Runtime session")?;
        if native_session_id.is_empty() {
            return Err("the public Runtime session has no provider-native identity");
        }
        if binding
            .native_session
            .as_deref()
            .is_some_and(|reviewed| reviewed != native_session_id)
        {
            return Err("the provider-native session identity changed before verification");
        }
        binding.native_session = Some(native_session_id.into());
        let workspace = active
            .workspaces
            .get(&task_id)
            .ok_or("the Task workspace evidence is unavailable")?;
        let capability_versions = task
            .capability_versions
            .iter()
            .map(|selection| parse_digest(&selection.version_sha256))
            .collect::<Result<Vec<_>, _>>()?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let run = snapshot
            .runs
            .iter()
            .rev()
            .find(|run| run.task_id == task_id && run.outcome.is_none())
            .ok_or("the Task has no active durable Run")?;
        let run_id = run.id;
        let policy_sha256 = run.policy_sha256;
        let gate_requests = task
            .gate_refs
            .iter()
            .map(|gate| {
                self.gates
                    .request(gate, run_id)
                    .map_err(|_| "a reviewed GateDefinition is unavailable")
            })
            .collect::<Result<Vec<_>, _>>()?;
        transition_task(
            &mut snapshot,
            task_id,
            TaskState::Running,
            TaskState::Verifying,
            format!("verify-{run_id}"),
        )?;
        ledger
            .put(&snapshot)
            .map_err(|_| "the verification intent could not be committed")?;
        Ok(VerificationIntent {
            mission_id,
            task_id,
            run_id,
            workspace: workspace.workspace.clone(),
            base_commit: workspace.base_commit.clone(),
            project_id: active.validated.project.worktree().as_str().into(),
            provider_runtime_id: binding.provider_runtime.clone(),
            native_session_id: native_session_id.into(),
            instruction_sha256: task.instruction.sha256,
            policy_sha256,
            output_roots: task.output_roots.clone(),
            gate_requests,
            capability_versions,
        })
    }

    fn runtime_session_id(
        &self,
        mission_id: &str,
        task_id: &str,
    ) -> Result<Box<str>, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        self.active
            .get(&mission_id)
            .and_then(|active| active.sessions.get(&task_id))
            .map(|binding| binding.runtime_session.clone())
            .ok_or("the Task has no public Runtime session")
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one evidence commit keeps Run, Gate, Artifact, Receipt, Task, and scheduler state atomic"
    )]
    fn commit_verification(
        &mut self,
        ledger: &Ledger,
        intent: &VerificationIntent,
        evidence: Result<VerificationEvidence, Vec<GateResult>>,
    ) -> Result<Response, &'static str> {
        let active = self
            .active
            .get_mut(&intent.mission_id)
            .ok_or("the Mission changed while verification ran")?;
        let scheduler = active
            .scheduler
            .as_mut()
            .ok_or("the Mission scheduler is unavailable")?;
        let mut snapshot = ledger
            .snapshot(intent.mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let run_index = snapshot
            .runs
            .iter()
            .position(|run| run.id == intent.run_id && run.outcome.is_none())
            .ok_or("the active Run changed while verification ran")?;
        let (passed, binary_fingerprint, artifacts, finish_tree, gates) = match evidence {
            Ok(evidence) => (
                evidence
                    .gates
                    .iter()
                    .all(|gate| gate.outcome == GateOutcome::Passed),
                Some(evidence.binary_fingerprint),
                evidence.artifacts,
                Some(evidence.finish_tree),
                evidence.gates,
            ),
            Err(gates) => (false, None, Vec::new(), None, gates),
        };
        let run = snapshot
            .runs
            .get_mut(run_index)
            .ok_or("the active Run is unavailable")?;
        if let Some(fingerprint) = binary_fingerprint {
            run.binary_fingerprint = Some(fingerprint);
        }
        for gate in &gates {
            snapshot.gate_runs.push(GateRunRecord {
                id: gate.request.gate_run_id,
                run_id: intent.run_id,
                gate_id: gate.request.definition.id.clone(),
                definition_sha256: gate.request.definition_sha256,
                outcome: gate_outcome(gate.outcome).into(),
                duration_ms: gate.duration_ms,
            });
        }
        if passed {
            let binary_fingerprint =
                binary_fingerprint.ok_or("passing verification has no provider binary identity")?;
            let finish_tree = finish_tree.ok_or("passing verification has no finish identity")?;
            let artifact_records = artifacts
                .iter()
                .map(|artifact| ArtifactRecord {
                    id: ArtifactId(artifact.sha256),
                    run_id: intent.run_id,
                    path: artifact.path.clone(),
                    sha256: artifact.sha256,
                    size: artifact.size,
                })
                .collect::<Vec<_>>();
            let gate_evidence = gates
                .iter()
                .map(|gate| GateEvidence {
                    id: gate.request.definition.id.clone(),
                    definition_sha256: gate.request.definition_sha256,
                    status: gate_outcome(gate.outcome).into(),
                })
                .collect();
            let (receipt_id, receipt) = Receipt::seal(ReceiptInput {
                mission_id: intent.mission_id,
                task_id: intent.task_id,
                run_id: intent.run_id,
                project_id: intent.project_id.clone(),
                instruction_sha256: intent.instruction_sha256,
                base_commit: intent.base_commit.clone(),
                finish_tree,
                provider_observation: ProviderObservation {
                    runtime_id: intent.provider_runtime_id.clone(),
                    binary_fingerprint,
                    model: None,
                    native_session_id: intent.native_session_id.clone(),
                },
                artifacts,
                gates: gate_evidence,
                capability_versions: intent.capability_versions.clone(),
                policy_sha256: intent.policy_sha256,
            })
            .map_err(|_| "the passing Receipt is incomplete")?;
            snapshot.artifacts.extend(artifact_records);
            snapshot.receipts.push((receipt_id, receipt));
            snapshot
                .runs
                .get_mut(run_index)
                .ok_or("the passing Run is unavailable")?
                .outcome = Some(RunOutcome::Passed);
            transition_task(
                &mut snapshot,
                intent.task_id,
                TaskState::Verifying,
                TaskState::Passed,
                format!("passed-{}", intent.run_id),
            )?;
            let effects = scheduler
                .pass(intent.run_id)
                .map_err(|_| "the scheduler refused passing evidence")?;
            for task in &active.validated.tasks {
                if scheduler.eligibility(&task.key)
                    == Some(runtrol_orchestrator::Eligibility::Ready)
                    && snapshot
                        .tasks
                        .iter()
                        .any(|record| record.id == task.id && record.state == TaskState::Pending)
                {
                    transition_task(
                        &mut snapshot,
                        task.id,
                        TaskState::Pending,
                        TaskState::Eligible,
                        format!("eligible-after-{}", intent.run_id),
                    )?;
                }
            }
            if effects
                .iter()
                .any(|effect| matches!(effect, SchedulerEffect::PresentIntegration))
            {
                snapshot
                    .mission
                    .transition(
                        format!("integrating-{}", intent.run_id).into(),
                        MissionState::Running,
                        MissionState::Integrating,
                    )
                    .map_err(|_| "the Mission cannot enter integration review")?;
            } else {
                reserve_available(&mut snapshot, scheduler)?;
            }
        } else {
            let failed_run = snapshot
                .runs
                .get_mut(run_index)
                .ok_or("the failed Run is unavailable")?;
            failed_run.outcome = Some(RunOutcome::Failed);
            let attempts = failed_run.attempt;
            let retryable = attempts < active.validated.spec.limits.max_runs_per_task;
            let next = if retryable {
                TaskState::Retryable
            } else {
                TaskState::Failed
            };
            transition_task(
                &mut snapshot,
                intent.task_id,
                TaskState::Verifying,
                next,
                format!("verification-failed-{}", intent.run_id),
            )?;
            if retryable {
                scheduler
                    .retry(intent.run_id)
                    .map_err(|_| "the scheduler refused the retryable result")?;
            } else {
                let effects = scheduler
                    .fail(intent.run_id)
                    .map_err(|_| "the scheduler refused the failed result")?;
                if active.validated.spec.limits.stop_on_critical_failure {
                    snapshot
                        .mission
                        .transition(
                            format!("failed-{}", intent.run_id).into(),
                            MissionState::Running,
                            MissionState::Failed,
                        )
                        .map_err(|_| "the Mission failure could not be committed")?;
                } else if effects
                    .iter()
                    .any(|effect| matches!(effect, SchedulerEffect::PresentIntegration))
                {
                    snapshot
                        .mission
                        .transition(
                            format!("integrating-{}", intent.run_id).into(),
                            MissionState::Running,
                            MissionState::Integrating,
                        )
                        .map_err(|_| "the Mission cannot enter integration review")?;
                } else if effects
                    .iter()
                    .any(|effect| matches!(effect, SchedulerEffect::FinishWithoutPassingResult))
                {
                    snapshot
                        .mission
                        .transition(
                            format!("no-passing-result-{}", intent.run_id).into(),
                            MissionState::Running,
                            MissionState::Failed,
                        )
                        .map_err(|_| "the Mission failure could not be committed")?;
                } else {
                    reserve_available(&mut snapshot, scheduler)?;
                }
            }
        }
        ledger
            .put(&snapshot)
            .map_err(|_| "the verification evidence could not be committed")?;
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&intent.mission_id),
        ))
    }

    fn integration_intent(
        &self,
        ledger: &Ledger,
        mission_id: &str,
        selected_task_id: Option<&str>,
    ) -> Result<IntegrationIntent, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let active = self
            .active
            .get(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if snapshot.mission.state != MissionState::Integrating {
            return Err("the Mission is not ready for integrated-tree verification");
        }
        let (selected_task, selected_receipt_id) = integration_selection(
            active.validated.spec.completion_policy,
            &snapshot,
            selected_task_id,
        )?;
        review_files_current(&active.validated, &snapshot.mission.mission_ref)?;
        if policy_digest(&self.gates, &active.validated) != snapshot.mission.policy_sha256 {
            return Err("the reviewed Mission Gate policy changed");
        }
        let receipts = snapshot
            .receipts
            .iter()
            .filter(|(_, receipt)| selected_task.is_none_or(|task_id| receipt.task_id == task_id));
        let mut artifacts = BTreeMap::new();
        let mut selected_run = None;
        for (_, receipt) in receipts {
            selected_run = Some(receipt.run_id);
            for artifact in &receipt.artifacts {
                if artifacts
                    .insert(artifact.path.clone(), artifact.clone())
                    .is_some_and(|prior| prior != *artifact)
                {
                    return Err("passing Task Receipts disagree about an integrated Artifact");
                }
            }
        }
        if artifacts.is_empty() {
            return Err("the Mission has no passing Artifact evidence to integrate");
        }
        let run_id = selected_run.ok_or("the Mission has no passing Run for integration Gates")?;
        let mut gate_ids: Vec<&str> = active
            .validated
            .tasks
            .iter()
            .filter(|task| selected_task.is_none_or(|task_id| task.id == task_id))
            .flat_map(|task| task.gate_refs.iter().map(AsRef::as_ref))
            .collect();
        gate_ids.sort_unstable();
        gate_ids.dedup();
        let gate_requests = gate_ids
            .into_iter()
            .map(|gate| {
                self.gates
                    .request(gate, run_id)
                    .map_err(|_| "an integration GateDefinition is unavailable")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IntegrationIntent {
            mission_id,
            project: active.validated.project.worktree().clone(),
            selected_task_id: selected_task,
            selected_receipt_id,
            expected_artifacts: artifacts.into_values().collect(),
            gate_requests,
        })
    }

    fn commit_integration(
        &mut self,
        ledger: &Ledger,
        intent: &IntegrationIntent,
        mut evidence: IntegrationEvidence,
    ) -> Result<Response, &'static str> {
        let active = self
            .active
            .get_mut(&intent.mission_id)
            .ok_or("the Mission changed while integration verification ran")?;
        let mut snapshot = ledger
            .snapshot(intent.mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if snapshot.mission.state != MissionState::Integrating {
            return Err("the Mission left integration review");
        }
        if !integration_artifacts_match(&intent.expected_artifacts, &mut evidence.artifacts)
            || evidence
                .gates
                .iter()
                .any(|gate| gate.outcome != GateOutcome::Passed)
            || evidence.gates.len() != intent.gate_requests.len()
        {
            return Err("the integrated tree does not match passing Task evidence and Gates");
        }
        for gate in evidence.gates {
            snapshot.gate_runs.push(GateRunRecord {
                id: gate.request.gate_run_id,
                run_id: gate.request.run_id,
                gate_id: gate.request.definition.id,
                definition_sha256: gate.request.definition_sha256,
                outcome: gate_outcome(gate.outcome).into(),
                duration_ms: gate.duration_ms,
            });
        }
        snapshot.mission.integration = Some(IntegrationRecord {
            selected_task_id: intent.selected_task_id,
            selected_receipt_id: intent.selected_receipt_id,
        });
        snapshot
            .mission
            .transition(
                "integration-passed".into(),
                MissionState::Integrating,
                MissionState::Completed,
            )
            .map_err(|_| "the Mission cannot complete integration")?;
        snapshot.compact();
        active.scheduler = None;
        ledger
            .put(&snapshot)
            .map_err(|_| "the integrated Mission completion could not be committed")?;
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&intent.mission_id),
        ))
    }

    fn retry_task(
        &mut self,
        ledger: &Ledger,
        mission_id: &str,
        task_id: &str,
    ) -> Result<Response, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let task_id: runtrol_ledger::TaskId = task_id
            .parse()
            .map_err(|_| "the Task identity is invalid")?;
        let active = self
            .active
            .get_mut(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let scheduler = active
            .scheduler
            .as_mut()
            .ok_or("the Mission scheduler is unavailable")?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        let retry_generation = snapshot.runs.len();
        let before = snapshot
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.state)
            .ok_or("the Task does not exist")?;
        if !matches!(before, TaskState::Retryable | TaskState::Blocked) {
            return Err("the Task is not retryable or recovery-blocked");
        }
        transition_task(
            &mut snapshot,
            task_id,
            before,
            TaskState::Eligible,
            format!("retry-{retry_generation}"),
        )?;
        active.sessions.remove(&task_id);
        scheduler
            .reopen(task_id)
            .map_err(|_| "the Task dependencies do not permit a retry")?;
        if snapshot.mission.state == MissionState::Running {
            reserve_available(&mut snapshot, scheduler)?;
        }
        ledger
            .put(&snapshot)
            .map_err(|_| "the Task retry could not be committed")?;
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&mission_id),
        ))
    }

    fn snapshot_response(snapshot: &LedgerSnapshot, active: Option<&ActiveMission>) -> Response {
        Response::Mission(Box::new(snapshot_of(snapshot, active)))
    }
}

fn load_gates(path: &AbsPath) -> Result<GateRegistry, String> {
    let metadata = match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GateRegistry::default());
        }
        Err(error) => return Err(format!("cannot inspect the Mission Gate registry: {error}")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("the Mission Gate registry is not a regular file".to_owned());
    }
    if metadata.len() > MAX_GATE_FILE_BYTES {
        return Err("the Mission Gate registry exceeds its byte bound".to_owned());
    }
    let bytes = std::fs::read(path.as_std_path())
        .map_err(|error| format!("cannot read the Mission Gate registry: {error}"))?;
    let stored: GateFile = serde_json::from_slice(&bytes)
        .map_err(|_| "the Mission Gate registry is malformed".to_owned())?;
    if stored.schema != GATE_FILE_SCHEMA {
        return Err("the Mission Gate registry schema is unsupported".to_owned());
    }
    let mut gates = GateRegistry::default();
    for definition in stored.definitions {
        gates
            .register(LocalScope::GateRegister, definition)
            .map_err(|_| "the Mission Gate registry contains an invalid definition".to_owned())?;
    }
    Ok(gates)
}

fn bind_durable_task_ids(
    validated: &mut ValidatedMission,
    snapshot: &LedgerSnapshot,
) -> Result<(), String> {
    if validated.tasks.len() != snapshot.tasks.len() {
        return Err("recovered Mission Task count changed".to_owned());
    }
    for task in &mut validated.tasks {
        let record = snapshot
            .tasks
            .iter()
            .find(|record| record.task_key == task.key)
            .ok_or_else(|| "a recovered Mission Task key changed".to_owned())?;
        if record.instruction_ref != task.instruction.path
            || record.instruction_sha256 != task.instruction.sha256
        {
            return Err("a recovered Mission Task instruction changed".to_owned());
        }
        task.id = record.id;
    }
    Ok(())
}

fn block_unrecoverable(ledger: &Ledger, snapshot: &mut LedgerSnapshot) -> Result<(), String> {
    if snapshot.mission.state == MissionState::Running {
        snapshot
            .mission
            .transition(
                "restart-contract-changed".into(),
                MissionState::Running,
                MissionState::Blocked,
            )
            .map_err(|_| "an unrecoverable Mission could not be blocked".to_owned())?;
        for task in &mut snapshot.tasks {
            if matches!(
                task.state,
                TaskState::Eligible
                    | TaskState::Reserved
                    | TaskState::AwaitingInput
                    | TaskState::Running
                    | TaskState::AwaitingApproval
                    | TaskState::Verifying
            ) && task.state.allows(TaskState::Blocked)
            {
                task.transition(
                    format!("restart-contract-changed-{}", task.id).into(),
                    task.state,
                    TaskState::Blocked,
                )
                .map_err(|_| "an unrecoverable Task could not be blocked".to_owned())?;
            }
        }
        ledger.put(snapshot).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn recover_workspaces(
    validated: &ValidatedMission,
    snapshot: &LedgerSnapshot,
) -> Result<BTreeMap<runtrol_ledger::TaskId, WorkspaceBinding>, String> {
    let mut recovered = BTreeMap::new();
    for record in &snapshot.tasks {
        let Some(workspace_id) = record.workspace_id.as_deref() else {
            if record.base_commit.is_some() || record.workspace_owned {
                return Err("a recovered Task has incomplete workspace evidence".to_owned());
            }
            continue;
        };
        let base_commit = record
            .base_commit
            .clone()
            .ok_or_else(|| "a recovered Task has no base commit".to_owned())?;
        let workspace = AbsPath::canonicalize(workspace_id)
            .map_err(|_| "a recovered Task workspace is unavailable".to_owned())?;
        let identity = ProjectIdentity::discover(workspace.clone())
            .map_err(|_| "a recovered Task workspace identity is unavailable".to_owned())?;
        let task = validated
            .tasks
            .iter()
            .find(|task| task.id == record.id)
            .ok_or_else(|| "a recovered Task workspace has no contract".to_owned())?;
        let valid = match task.workspace_mode {
            WorkspaceMode::ReadOnlyBase => identity.worktree() == validated.project.worktree(),
            WorkspaceMode::IsolatedWorktree => {
                record.workspace_owned
                    && identity.worktree() != validated.project.worktree()
                    && identity.common_store() == validated.project.common_store()
            }
        };
        if !valid {
            return Err("a recovered Task workspace changed identity".to_owned());
        }
        recovered.insert(
            record.id,
            WorkspaceBinding {
                workspace,
                base_commit,
            },
        );
    }
    Ok(recovered)
}

fn save_gates(path: &AbsPath, gates: &GateRegistry) -> Result<(), String> {
    let bytes = serde_json::to_vec(&GateFile {
        schema: GATE_FILE_SCHEMA,
        definitions: gates.definitions().cloned().collect(),
    })
    .map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_GATE_FILE_BYTES) {
        return Err("the bounded Mission Gate registry is full".to_owned());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the Mission Gate registry has no parent".to_owned())?;
    let temporary = parent
        .join("mission-gates.json.writing")
        .map_err(|error| error.to_string())?;
    let mut file =
        std::fs::File::create(temporary.as_std_path()).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    std::fs::rename(temporary.as_std_path(), path.as_std_path()).map_err(|error| error.to_string())
}

/// Prepare one exact Task worktree through fixed Git argument vectors and daemon-owned containment.
pub(crate) async fn prepare_workspace(
    controller: &tokio::sync::Mutex<MissionController>,
    ledger: &Ledger,
    containment: &Containment,
    mission_id: &str,
    task_id: &str,
) -> Response {
    let intent = {
        let controller = controller.lock().await;
        match controller.workspace_intent(ledger, mission_id, task_id) {
            Ok(WorkspacePreparation::Ready(response)) => return response,
            Ok(WorkspacePreparation::Run(intent)) => intent,
            Err(message) => return failed(message),
        }
    };
    match run_workspace_preparation(&intent, containment).await {
        Ok((workspace, base_commit)) => {
            let mut controller = controller.lock().await;
            controller
                .commit_workspace(ledger, &intent, workspace, base_commit)
                .unwrap_or_else(failed)
        }
        Err(message) => failed(message),
    }
}

/// Seal declared Artifact metadata and run fixed Gates for one exact durable Run.
pub(crate) async fn prepare_verification(
    composed: &crate::compose::Composed,
    mission_id: &str,
    task_id: &str,
) -> Response {
    let runtime_session = {
        let controller = composed.missions.lock().await;
        match controller.runtime_session_id(mission_id, task_id) {
            Ok(session) => session,
            Err(message) => return failed(message),
        }
    };
    let Ok(runtime_session) = runtime_session.parse::<runtrol_provider::SessionId>() else {
        return failed("the Task public Runtime session identity is invalid");
    };
    let native_session = match composed.store.get_session(runtime_session) {
        Ok(Some(session)) => session.native.to_string(),
        Ok(None) => return failed("the Task public Runtime session has no durable native pointer"),
        Err(_) => return failed("the Task public Runtime session pointer cannot be read"),
    };
    let intent = {
        let mut controller = composed.missions.lock().await;
        match controller.verification_intent(&composed.ledger, mission_id, task_id, &native_session)
        {
            Ok(intent) => intent,
            Err(message) => return failed(message),
        }
    };
    let Ok(provider) = runtrol_provider::ProviderId::parse(&intent.provider_runtime_id) else {
        return commit_verification_failure(composed, &intent, Vec::new()).await;
    };
    let Ok(prepared) = crate::provider_prepare::prepared_driver(composed, provider).await else {
        return commit_verification_failure(composed, &intent, Vec::new()).await;
    };
    let binary_fingerprint = prepared.binary_identity;
    drop(prepared.driver);
    let artifact_intent = intent.clone();
    let Ok(Ok(artifacts)) =
        tokio::task::spawn_blocking(move || collect_artifacts(&artifact_intent)).await
    else {
        return commit_verification_failure(composed, &intent, Vec::new()).await;
    };
    let finish_tree = artifact_identity(&artifacts);
    let gates = execute_gates(
        &composed.containment,
        &intent.workspace,
        &intent.gate_requests,
    )
    .await;
    let evidence = VerificationEvidence {
        binary_fingerprint,
        artifacts,
        finish_tree,
        gates,
    };
    let mut controller = composed.missions.lock().await;
    controller
        .commit_verification(&composed.ledger, &intent, Ok(evidence))
        .unwrap_or_else(failed)
}

async fn execute_gates(
    containment: &Containment,
    workspace: &AbsPath,
    requests: &[GateRequest],
) -> Vec<GateResult> {
    let mut gates = Vec::with_capacity(requests.len());
    for request in requests {
        let started = Instant::now();
        let outcome = match resolve(&request.definition.program) {
            Ok(program) => {
                let arguments = request
                    .definition
                    .arguments
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                match capture_in(
                    &program,
                    &arguments,
                    workspace,
                    Duration::from_millis(request.definition.timeout_ms),
                    containment,
                )
                .await
                {
                    Ok(output) if output.succeeded() => GateOutcome::Passed,
                    Ok(_) => GateOutcome::Failed,
                    Err(SpawnError::Timeout { .. }) => GateOutcome::TimedOut,
                    Err(_) => GateOutcome::LaunchFailed,
                }
            }
            Err(_) => GateOutcome::LaunchFailed,
        };
        gates.push(GateResult {
            request: request.clone(),
            outcome,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
    }
    gates
}

/// Verify the manually integrated project tree against passing Task Receipts and fixed Gates.
pub(crate) async fn prepare_integration(
    composed: &crate::compose::Composed,
    mission_id: &str,
    selected_task_id: Option<&str>,
) -> Response {
    let intent = {
        let controller = composed.missions.lock().await;
        match controller.integration_intent(&composed.ledger, mission_id, selected_task_id) {
            Ok(intent) => intent,
            Err(message) => return failed(message),
        }
    };
    let workspace = intent.project.clone();
    let roots = intent
        .expected_artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect::<Vec<_>>();
    let Ok(Ok(mut artifacts_before)) =
        tokio::task::spawn_blocking(move || collect_artifacts_at(&workspace, &roots)).await
    else {
        return failed("the integrated Artifact tree could not be sealed");
    };
    if !integration_artifacts_match(&intent.expected_artifacts, &mut artifacts_before) {
        return failed("the integrated tree does not match passing Task evidence before Gates");
    }
    let gates = execute_gates(
        &composed.containment,
        &intent.project,
        &intent.gate_requests,
    )
    .await;
    let workspace = intent.project.clone();
    let roots = intent
        .expected_artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect::<Vec<_>>();
    let Ok(Ok(artifacts)) =
        tokio::task::spawn_blocking(move || collect_artifacts_at(&workspace, &roots)).await
    else {
        return failed("the integrated Artifact tree could not be resealed after Gates");
    };
    let mut controller = composed.missions.lock().await;
    controller
        .commit_integration(
            &composed.ledger,
            &intent,
            IntegrationEvidence { artifacts, gates },
        )
        .unwrap_or_else(failed)
}

fn integration_artifacts_match(
    expected: &[ArtifactEvidence],
    actual: &mut [ArtifactEvidence],
) -> bool {
    actual.sort_by(|left, right| left.path.cmp(&right.path));
    actual == expected
}

async fn commit_verification_failure(
    composed: &crate::compose::Composed,
    intent: &VerificationIntent,
    gates: Vec<GateResult>,
) -> Response {
    let mut controller = composed.missions.lock().await;
    controller
        .commit_verification(&composed.ledger, intent, Err(gates))
        .unwrap_or_else(failed)
}

fn collect_artifacts(intent: &VerificationIntent) -> Result<Vec<ArtifactEvidence>, &'static str> {
    collect_artifacts_at(&intent.workspace, &intent.output_roots)
}

fn collect_artifacts_at(
    workspace: &AbsPath,
    output_roots: &[Box<str>],
) -> Result<Vec<ArtifactEvidence>, &'static str> {
    let mut files = Vec::new();
    for root in output_roots {
        let path = workspace
            .join(root)
            .map_err(|_| "a declared Artifact path is invalid")?;
        collect_files(path.as_std_path(), &mut files)?;
    }
    files.sort();
    files.dedup();
    if files.is_empty() || files.len() > runtrol_ledger::MAX_ARTIFACTS_PER_RUN {
        return Err("the declared Artifact set is empty or exceeds its bound");
    }
    let mut total = 0_u64;
    let mut artifacts = Vec::with_capacity(files.len());
    for path in files {
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| "a declared Artifact is unavailable")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("a declared Artifact is not a regular file");
        }
        total = total
            .checked_add(metadata.len())
            .ok_or("the declared Artifact bytes overflowed")?;
        if total > runtrol_ledger::MAX_ARTIFACT_BYTES_PER_RUN {
            return Err("the declared Artifact bytes exceed the evidence bound");
        }
        let relative = path
            .strip_prefix(workspace.as_std_path())
            .map_err(|_| "a declared Artifact escaped its Task workspace")?;
        let relative = relative
            .to_str()
            .ok_or("a declared Artifact path is not UTF-8")?
            .replace('\\', "/");
        let sha256 = hash_file(&path)?;
        artifacts.push(ArtifactEvidence {
            path: relative.into(),
            sha256,
            size: metadata.len(),
        });
    }
    Ok(artifacts)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), &'static str> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "a declared Artifact root is unavailable")?;
    if metadata.file_type().is_symlink() {
        return Err("a declared Artifact root cannot be a symbolic link");
    }
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err("a declared Artifact root is not a file or directory");
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|_| "a declared Artifact directory cannot be read")?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "a declared Artifact directory entry cannot be read")?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        collect_files(&entry.path(), files)?;
        if files.len() > runtrol_ledger::MAX_ARTIFACTS_PER_RUN {
            return Err("the declared Artifact count exceeds the evidence bound");
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<[u8; 32], &'static str> {
    let mut file = std::fs::File::open(path).map_err(|_| "a declared Artifact cannot be opened")?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "a declared Artifact cannot be read")?;
        if read == 0 {
            break;
        }
        let bytes = buffer
            .get(..read)
            .ok_or("the Artifact read exceeded its fixed buffer")?;
        hasher.update(bytes);
    }
    Ok(hasher.finalize().into())
}

fn artifact_identity(artifacts: &[ArtifactEvidence]) -> Box<str> {
    let mut hasher = Sha256::new();
    for artifact in artifacts {
        let path_len = u64::try_from(artifact.path.len()).unwrap_or(u64::MAX);
        hasher.update(path_len.to_be_bytes());
        hasher.update(artifact.path.as_bytes());
        hasher.update(artifact.sha256);
        hasher.update(artifact.size.to_be_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    format!("snapshot:{}", hex(&digest)).into()
}

fn current_unix_ms() -> Result<u64, &'static str> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "the system clock cannot issue a Mission approval")?;
    u64::try_from(elapsed.as_millis()).map_err(|_| "the system clock exceeded the approval range")
}

fn approval_deadline() -> Result<u64, &'static str> {
    let now = current_unix_ms()?;
    let window = u64::try_from(MISSION_START_APPROVAL_WINDOW.as_millis())
        .map_err(|_| "the Mission approval window exceeded its bound")?;
    now.checked_add(window)
        .ok_or("the Mission approval deadline exceeded its bound")
}

async fn run_workspace_preparation(
    intent: &WorkspaceIntent,
    containment: &Containment,
) -> Result<(AbsPath, Box<str>), &'static str> {
    let git = resolve("git").map_err(|_| "Git is unavailable for Mission workspace preparation")?;
    if intent.require_clean_base {
        let status = capture(
            &git,
            &[
                "-C".to_owned(),
                intent.base_worktree.as_str().to_owned(),
                "status".to_owned(),
                "--porcelain=v1".to_owned(),
                "--untracked-files=all".to_owned(),
            ],
            Duration::from_secs(15),
            containment,
        )
        .await
        .map_err(|_| "Git could not inspect the reviewed Mission base")?;
        if !status.succeeded() || status.truncated {
            return Err("Git could not prove the reviewed Mission base is clean");
        }
        if !status.stdout.is_empty() {
            return Err("the reviewed Mission requires a clean base worktree");
        }
    }
    let base_commit = resolve_base_with(&git, intent, containment).await?;
    if !intent.isolated {
        let head = resolve_revision(&git, &intent.base_worktree, "HEAD", containment).await?;
        if head != base_commit {
            return Err("the read-only Task base reference is not the checked-out base");
        }
        return Ok((intent.base_worktree.clone(), base_commit));
    }
    if intent.target.as_std_path().exists() {
        return Err("the Mission worktree target already exists without an ownership record");
    }
    let parent = intent
        .target
        .parent()
        .ok_or("the Mission worktree target has no parent")?;
    std::fs::create_dir_all(parent.as_std_path())
        .map_err(|_| "the Mission worktree parent could not be created")?;
    let added = capture(
        &git,
        &[
            "-C".to_owned(),
            intent.base_worktree.as_str().to_owned(),
            "worktree".to_owned(),
            "add".to_owned(),
            "--detach".to_owned(),
            intent.target.as_str().to_owned(),
            base_commit.to_string(),
        ],
        Duration::from_mins(1),
        containment,
    )
    .await
    .map_err(|_| "Git could not create the Mission worktree")?;
    if !added.succeeded() || added.truncated {
        return Err("Git refused to create the Mission worktree");
    }
    let workspace = AbsPath::canonicalize(intent.target.as_str())
        .map_err(|_| "the created Mission worktree is unavailable")?;
    let identity = ProjectIdentity::discover(workspace.clone())
        .map_err(|_| "the created Mission worktree identity is unavailable")?;
    if identity.worktree() == &intent.base_worktree
        || identity.common_store() != &intent.common_store
    {
        return Err("Git created a workspace outside the reviewed project identity");
    }
    Ok((workspace, base_commit))
}

async fn resolve_base_with(
    git: &runtrol_childproc::Program,
    intent: &WorkspaceIntent,
    containment: &Containment,
) -> Result<Box<str>, &'static str> {
    let revision = format!("{}^{{commit}}", intent.base_ref);
    resolve_revision(git, &intent.base_worktree, &revision, containment).await
}

async fn resolve_revision(
    git: &runtrol_childproc::Program,
    worktree: &AbsPath,
    revision: &str,
    containment: &Containment,
) -> Result<Box<str>, &'static str> {
    let output = capture(
        git,
        &[
            "-C".to_owned(),
            worktree.as_str().to_owned(),
            "rev-parse".to_owned(),
            "--verify".to_owned(),
            "--end-of-options".to_owned(),
            revision.to_owned(),
        ],
        Duration::from_secs(15),
        containment,
    )
    .await
    .map_err(|_| "Git could not resolve the reviewed Mission base")?;
    if !output.succeeded() || output.truncated {
        return Err("the reviewed Mission base reference is unavailable");
    }
    let text = core::str::from_utf8(&output.stdout)
        .map_err(|_| "Git returned a non-UTF-8 base identity")?
        .trim();
    if !matches!(text.len(), 40 | 64) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Git returned an invalid base commit identity");
    }
    Ok(text.to_ascii_lowercase().into())
}

fn workspace_response(
    mission_id: MissionId,
    task_id: runtrol_ledger::TaskId,
    binding: &WorkspaceBinding,
) -> Response {
    Response::MissionWorkspace(Box::new(MissionWorkspace {
        mission_id: mission_id.to_string().into(),
        task_id: task_id.to_string().into(),
        workspace: binding.workspace.as_str().into(),
        base_commit: binding.base_commit.clone(),
    }))
}

fn parse_digest(value: &str) -> Result<[u8; 32], &'static str> {
    let value = value.strip_prefix("cpv_").unwrap_or(value);
    if value.len() != 64 {
        return Err("a capability version digest is invalid");
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = core::str::from_utf8(pair).map_err(|_| "a capability digest is not UTF-8")?;
        let byte = digest
            .get_mut(index)
            .ok_or("a capability version digest is too long")?;
        *byte = u8::from_str_radix(pair, 16)
            .map_err(|_| "a capability version digest is not lowercase hexadecimal")?;
    }
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err("a capability version digest is not lowercase hexadecimal");
    }
    Ok(digest)
}

const fn gate_outcome(outcome: GateOutcome) -> &'static str {
    match outcome {
        GateOutcome::Passed => "passed",
        GateOutcome::Failed => "failed",
        GateOutcome::TimedOut => "timedOut",
        GateOutcome::Cancelled => "cancelled",
        GateOutcome::LaunchFailed => "launchFailed",
    }
}

fn ensure_review_current(
    validated: &ValidatedMission,
    mission_ref: &str,
    approved_capabilities: &[CapabilitySelection],
) -> Result<(), &'static str> {
    if validated.tasks.iter().any(|task| {
        task.capability_versions
            .iter()
            .any(|selection| !approved_capabilities.contains(selection))
    }) {
        return Err("a selected capability is no longer exactly approved and active");
    }
    review_files_current(validated, mission_ref)
}

fn review_files_current(
    validated: &ValidatedMission,
    mission_ref: &str,
) -> Result<(), &'static str> {
    let mission = read_review_file(validated.project.worktree(), mission_ref, MAX_MISSION_BYTES)?;
    if Sha256::digest(&mission).as_slice() != validated.mission_sha256 {
        return Err("the Mission file bytes changed after review");
    }
    for task in &validated.tasks {
        let instruction = read_review_file(
            validated.project.worktree(),
            &task.instruction.path,
            MAX_INSTRUCTION_BYTES,
        )?;
        if Sha256::digest(instruction).as_slice() != task.instruction.sha256 {
            return Err("a Task instruction file changed after review");
        }
    }
    Ok(())
}

fn read_review_file(root: &AbsPath, relative: &str, limit: usize) -> Result<Vec<u8>, &'static str> {
    if !safe_relative(relative) {
        return Err("a reviewed project file path is invalid");
    }
    let path = root
        .join(relative)
        .map_err(|_| "a reviewed project file path is invalid")?;
    let canonical = AbsPath::canonicalize(path.as_str())
        .map_err(|_| "a reviewed project file is unavailable")?;
    if !canonical.is_under(root) {
        return Err("a reviewed project file escaped the project");
    }
    let metadata = std::fs::symlink_metadata(canonical.as_std_path())
        .map_err(|_| "a reviewed project file is unavailable")?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || usize::try_from(metadata.len()).map_or(true, |size| size > limit)
    {
        return Err("a reviewed project file is not a bounded regular file");
    }
    let file = std::fs::File::open(canonical.as_std_path())
        .map_err(|_| "a reviewed project file cannot be read")?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "a reviewed project file cannot be read")?;
    if bytes.len() > limit {
        return Err("a reviewed project file exceeds its byte bound");
    }
    Ok(bytes)
}

fn integration_selection(
    policy: CompletionPolicy,
    snapshot: &LedgerSnapshot,
    requested_task_id: Option<&str>,
) -> Result<(Option<TaskId>, Option<ReceiptId>), &'static str> {
    let selected_task = match policy {
        CompletionPolicy::AllTasks => {
            if requested_task_id.is_some()
                || snapshot
                    .tasks
                    .iter()
                    .any(|task| task.state != TaskState::Passed)
            {
                return Err("the all-Task Mission requires every Task to pass without a selection");
            }
            None
        }
        CompletionPolicy::ChooseOne => {
            let selected: TaskId = requested_task_id
                .ok_or("the comparison Mission requires one selected passing Task")?
                .parse()
                .map_err(|_| "the selected Task identity is invalid")?;
            if !snapshot
                .tasks
                .iter()
                .any(|task| task.id == selected && task.state == TaskState::Passed)
            {
                return Err("the selected comparison Task did not pass");
            }
            Some(selected)
        }
    };
    let selected_receipt = selected_task
        .map(|task_id| {
            snapshot
                .receipts
                .iter()
                .rev()
                .find(|(_, receipt)| receipt.task_id == task_id)
                .map(|(receipt_id, _)| *receipt_id)
                .ok_or("the selected comparison Task has no passing Receipt")
        })
        .transpose()?;
    Ok((selected_task, selected_receipt))
}

fn policy_digest(gates: &GateRegistry, validated: &ValidatedMission) -> [u8; 32] {
    let mut gate_ids: Vec<&str> = validated
        .tasks
        .iter()
        .flat_map(|task| task.gate_refs.iter().map(AsRef::as_ref))
        .collect();
    gate_ids.sort_unstable();
    gate_ids.dedup();
    let mut hasher = Sha256::new();
    hasher.update(validated.mission_sha256);
    for gate_id in gate_ids {
        if let Some(definition) = gates.get(gate_id) {
            hasher.update(definition.digest());
        }
        hasher.update(
            u64::try_from(gate_id.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(gate_id.as_bytes());
    }
    hasher.finalize().into()
}

fn reserve_available(
    snapshot: &mut LedgerSnapshot,
    scheduler: &mut Scheduler,
) -> Result<(), &'static str> {
    loop {
        match scheduler.reserve_next() {
            Ok(SchedulerEffect::PrepareSession(reservation)) => transition_task(
                snapshot,
                reservation.task_id,
                TaskState::Eligible,
                TaskState::Reserved,
                format!("reserved-{}", reservation.run_id),
            )?,
            Ok(_) => return Err("the scheduler emitted an invalid reservation effect"),
            Err(SchedulerError::NothingReady | SchedulerError::SlotsExhausted) => return Ok(()),
            Err(_) => return Err("the Mission scheduler refused Task reservation"),
        }
    }
}

fn reserve_recovered_work(
    snapshot: &mut LedgerSnapshot,
    scheduler: Option<&mut Scheduler>,
) -> Result<(), &'static str> {
    if snapshot.mission.state == MissionState::Running
        && let Some(scheduler) = scheduler
    {
        reserve_available(snapshot, scheduler)?;
    }
    Ok(())
}

fn transition_task(
    snapshot: &mut LedgerSnapshot,
    task_id: runtrol_ledger::TaskId,
    before: TaskState,
    after: TaskState,
    event: impl Into<Box<str>>,
) -> Result<(), &'static str> {
    let task = snapshot
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or("the Task is absent from the durable snapshot")?;
    task.transition(event.into(), before, after)
        .map_err(|_| "the Task state transition is not allowed")?;
    Ok(())
}

fn snapshot_of(snapshot: &LedgerSnapshot, active: Option<&ActiveMission>) -> MissionSnapshot {
    MissionSnapshot {
        mission: line_of(snapshot, active),
        mission_sha256: hex(&snapshot.mission.mission_sha256).into(),
        mission_ref: snapshot.mission.mission_ref.clone(),
        policy_sha256: hex(&snapshot.mission.policy_sha256).into(),
        approval_expires_unix_ms: snapshot.mission.approval_expires_unix_ms,
        integration: snapshot.mission.integration.as_ref().map(|integration| {
            MissionIntegrationLine {
                selected_task_id: integration
                    .selected_task_id
                    .map(|task_id| task_id.to_string().into()),
                selected_receipt_id: integration
                    .selected_receipt_id
                    .map(|receipt_id| receipt_id.to_string().into()),
            }
        }),
        tasks: snapshot
            .tasks
            .iter()
            .map(|task| task_line(task, snapshot, active))
            .collect(),
    }
}

fn line_of(snapshot: &LedgerSnapshot, active: Option<&ActiveMission>) -> MissionLine {
    let passed = snapshot
        .tasks
        .iter()
        .filter(|task| task.state == TaskState::Passed)
        .count();
    let awaiting = snapshot
        .tasks
        .iter()
        .filter(|task| task.state == TaskState::AwaitingInput)
        .count();
    MissionLine {
        mission_id: snapshot.mission.id.to_string().into(),
        name: snapshot.mission.display_name.clone(),
        project: snapshot.mission.project_id.clone(),
        state: mission_state(snapshot.mission.state).into(),
        completion_policy: active.map_or_else(
            || "unavailableAfterRestart".into(),
            |active| match active.validated.spec.completion_policy {
                CompletionPolicy::AllTasks => "allTasks".into(),
                CompletionPolicy::ChooseOne => "chooseOne".into(),
            },
        ),
        passed_tasks: u16::try_from(passed).unwrap_or(u16::MAX),
        total_tasks: u16::try_from(snapshot.tasks.len()).unwrap_or(u16::MAX),
        awaiting_input: u16::try_from(awaiting).unwrap_or(u16::MAX),
        schedule: snapshot.mission.schedule.as_ref().map(schedule_line),
    }
}

fn task_line(
    task: &TaskRecord,
    snapshot: &LedgerSnapshot,
    active: Option<&ActiveMission>,
) -> MissionTaskLine {
    let resolved = active.and_then(|active| {
        active
            .validated
            .tasks
            .iter()
            .find(|candidate| candidate.id == task.id)
    });
    let binding = active.and_then(|active| active.sessions.get(&task.id));
    let workspace = active.and_then(|active| active.workspaces.get(&task.id));
    let run_ids: Vec<_> = snapshot
        .runs
        .iter()
        .filter(|run| run.task_id == task.id)
        .map(|run| run.id)
        .collect();
    let passed_gates = snapshot
        .gate_runs
        .iter()
        .filter(|gate| run_ids.contains(&gate.run_id) && gate.outcome.as_ref() == "passed")
        .count();
    let failed_gates = snapshot
        .gate_runs
        .iter()
        .filter(|gate| run_ids.contains(&gate.run_id) && gate.outcome.as_ref() != "passed")
        .count();
    let receipt = snapshot
        .receipts
        .iter()
        .rev()
        .find(|(_, receipt)| receipt.task_id == task.id);
    MissionTaskLine {
        task_id: task.id.to_string().into(),
        key: resolved.map_or_else(|| task.id.to_string().into(), |task| task.key.clone()),
        state: task_state(task.state).into(),
        instruction_ref: task.instruction_ref.clone(),
        instruction_sha256: hex(&task.instruction_sha256).into(),
        workspace_mode: resolved.map_or_else(
            || "unavailableAfterRestart".into(),
            |task| match task.workspace_mode {
                WorkspaceMode::ReadOnlyBase => "readOnlyBase".into(),
                WorkspaceMode::IsolatedWorktree => "isolatedWorktree".into(),
            },
        ),
        provider_selector: resolved.map_or_else(
            || "unavailableAfterRestart".into(),
            |task| match &task.provider_selector {
                ProviderSelector::OperatorChoice => "operatorChoice".into(),
                ProviderSelector::Exact(id) => id.clone(),
            },
        ),
        output_roots: resolved.map_or_else(Vec::new, |task| task.output_roots.clone()),
        artifact_paths: receipt.map_or_else(Vec::new, |(_, receipt)| {
            receipt
                .artifacts
                .iter()
                .map(|artifact| artifact.path.clone())
                .collect()
        }),
        artifacts: receipt.map_or_else(Vec::new, |(_, receipt)| {
            receipt
                .artifacts
                .iter()
                .map(|artifact| MissionArtifactLine {
                    path: artifact.path.clone(),
                    size: artifact.size,
                    sha256: hex(&artifact.sha256).into(),
                })
                .collect()
        }),
        gate_refs: resolved.map_or_else(Vec::new, |task| task.gate_refs.clone()),
        capability_versions: resolved.map_or_else(Vec::new, |task| {
            task.capability_versions
                .iter()
                .map(|selection| MissionCapabilityLine {
                    capability_id: selection.capability_id.clone(),
                    version_sha256: selection.version_sha256.clone(),
                })
                .collect()
        }),
        session_id: binding.map(|binding| binding.runtime_session.clone()),
        workspace: workspace.map(|binding| binding.workspace.as_str().into()),
        base_commit: workspace.map(|binding| binding.base_commit.clone()),
        receipt_id: receipt.map(|(id, _)| id.to_string().into()),
        run_id: receipt.map(|(_, receipt)| receipt.run_id.to_string().into()),
        passed_gates: u16::try_from(passed_gates).unwrap_or(u16::MAX),
        failed_gates: u16::try_from(failed_gates).unwrap_or(u16::MAX),
    }
}

fn mission_state(state: MissionState) -> &'static str {
    match state {
        MissionState::Draft => "draft",
        MissionState::Validated => "validated",
        MissionState::Ready => "ready",
        MissionState::Running => "running",
        MissionState::Paused => "paused",
        MissionState::Blocked => "blocked",
        MissionState::Integrating => "integrating",
        MissionState::Completed => "completed",
        MissionState::Failed => "failed",
        MissionState::Cancelled => "cancelled",
        MissionState::Archived => "archived",
        MissionState::Rejected => "rejected",
    }
}

fn task_state(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Eligible => "eligible",
        TaskState::Reserved => "reserved",
        TaskState::AwaitingInput => "awaitingInput",
        TaskState::Running => "running",
        TaskState::AwaitingApproval => "awaitingApproval",
        TaskState::Verifying => "verifying",
        TaskState::Retryable => "retryable",
        TaskState::Blocked => "blocked",
        TaskState::Passed => "passed",
        TaskState::Skipped => "skipped",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
}
fn hex(digest: &[u8; 32]) -> String {
    use core::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ignored = write!(&mut text, "{byte:02x}");
    }
    text
}
fn failed(message: &'static str) -> Response {
    Response::Failed(runtrol_ipc::wire::WireError {
        message: message.into(),
        retryable: false,
        needs_the_operator: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        root: std::path::PathBuf,
        project: AbsPath,
        ledger: Ledger,
    }

    impl Scratch {
        fn make() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-mission-controller-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear scratch");
            }
            std::fs::create_dir_all(root.join("project/instructions")).expect("create project");
            let project = AbsPath::canonicalize(
                root.join("project")
                    .to_str()
                    .expect("UTF-8 scratch project"),
            )
            .expect("canonical project");
            let ledger_path = AbsPath::canonicalize(root.to_str().expect("UTF-8 scratch"))
                .expect("canonical scratch")
                .join("mission.redb")
                .expect("ledger path");
            let ledger = Ledger::open(&ledger_path).expect("open ledger");
            Self {
                root,
                project,
                ledger,
            }
        }

        fn write_mission(&self) -> [u8; 32] {
            use core::fmt::Write as _;
            let instruction = "preserve CRLF\r\nand UTF-8 한글\n".as_bytes();
            std::fs::write(
                self.project.as_std_path().join("instructions/task.md"),
                instruction,
            )
            .expect("write instruction");
            let digest: [u8; 32] = Sha256::digest(instruction).into();
            let mut digest_text = String::with_capacity(64);
            for byte in digest {
                write!(&mut digest_text, "{byte:02x}").expect("String write");
            }
            let mission = format!(
                r#"schema = "runtrol.dev/mission/v1alpha1"
name = "single task fixture"
project_id = "fixture-project"
base_ref = "main"
require_clean_base = true

[limits]
max_parallel_tasks = 1
max_hot_providers = 1
max_runs_per_task = 2
max_repair_cycles = 1
stop_on_critical_failure = true

[[tasks]]
id = "investigate"
instruction_ref = "instructions/task.md"
instruction_sha256 = "{digest_text}"
workspace_mode = "read_only_base"
provider_selector = "operator_choice"
output_roots = [".runtrol/handoffs/report"]
gate_refs = ["fixture-check"]
"#
            );
            std::fs::write(self.project.as_std_path().join("mission.toml"), mission)
                .expect("write Mission");
            digest
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn registered_gates_are_listed_in_stable_identity_order() {
        // The listing exists so a fan-out flow can offer the registered gates instead of making
        // the operator retype an identity from memory. Ids, programs, timeouts, nothing else.
        let scratch = Scratch::make();
        let mut controller = MissionController::default();
        for (gate_id, timeout_ms) in [("zz-later", 2_000), ("aa-first", 1_000)] {
            assert!(matches!(
                controller.answer(
                    &scratch.ledger,
                    &[],
                    &[],
                    &Request::MissionRegisterGate {
                        gate_id: gate_id.into(),
                        program: "fixture".into(),
                        arguments: Vec::new(),
                        timeout_ms,
                    },
                ),
                Response::Done
            ));
        }
        let listed = controller.answer(&scratch.ledger, &[], &[], &Request::MissionListGates);
        let Response::MissionGates(gates) = listed else {
            panic!("expected the gate listing, got {listed:?}");
        };
        assert_eq!(
            gates
                .iter()
                .map(|gate| gate.gate_id.as_ref())
                .collect::<Vec<_>>(),
            vec!["aa-first", "zz-later"],
            "stable identity order, not registration order"
        );
        let first = gates.first().expect("the listing holds the first gate");
        assert_eq!(first.program.as_ref(), "fixture");
        assert_eq!(first.timeout_ms, 1_000);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the product fixture covers review, public session binding, exact Send, Gates, and Receipt"
    )]
    fn local_send_returns_exact_reviewed_bytes_after_public_session_binding() {
        let scratch = Scratch::make();
        let instruction_digest = scratch.write_mission();
        let mut controller = MissionController::default();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let validated = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        );
        let Response::Mission(validated) = validated else {
            panic!("expected validated Mission");
        };
        let mission_id = validated.mission.mission_id.clone();
        let digest = validated.mission_sha256.clone();
        let task_id = validated.tasks.first().expect("one Task").task_id.clone();
        let started = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionStart {
                mission_id: mission_id.clone(),
                mission_sha256: digest,
            },
        );
        assert!(matches!(started, Response::Mission(_)));
        let WorkspacePreparation::Run(intent) = controller
            .workspace_intent(&scratch.ledger, &mission_id, &task_id)
            .expect("workspace intent")
        else {
            panic!("expected new workspace intent");
        };
        let prepared = controller
            .commit_workspace(
                &scratch.ledger,
                &intent,
                scratch.project.clone(),
                "fixture-base".into(),
            )
            .expect("prepare workspace");
        assert!(matches!(prepared, Response::MissionWorkspace(_)));
        let runtime_session = runtrol_provider::SessionId::now().to_string();
        let bound = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionBindSession {
                mission_id: mission_id.clone(),
                task_id: task_id.clone(),
                session_id: runtime_session.clone().into(),
                provider_runtime_id: "fixture-provider".into(),
                native_session_id: None,
                workspace: scratch.project.as_str().into(),
            },
        );
        assert!(matches!(bound, Response::Mission(_)));
        let sent = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionSendTaskInstruction {
                mission_id,
                task_id,
                instruction_sha256: hex(&instruction_digest).into(),
            },
        );
        let Response::MissionInstruction(sent) = sent else {
            panic!("expected exact instruction");
        };
        assert_eq!(
            sent.instruction.as_ref(),
            "preserve CRLF\r\nand UTF-8 한글\n"
        );
        assert_eq!(sent.session_id.as_ref(), runtime_session);
        let intent = controller
            .verification_intent(
                &scratch.ledger,
                &sent.mission_id,
                &sent.task_id,
                "native-fixture",
            )
            .expect("verification intent");
        assert_eq!(intent.native_session_id.as_ref(), "native-fixture");
        let gate = intent.gate_requests.first().expect("one Gate").clone();
        let verified = controller
            .commit_verification(
                &scratch.ledger,
                &intent,
                Ok(VerificationEvidence {
                    binary_fingerprint: [9; 32],
                    artifacts: vec![ArtifactEvidence {
                        path: ".runtrol/handoffs/report/result.txt".into(),
                        sha256: [8; 32],
                        size: 12,
                    }],
                    finish_tree: "snapshot:fixture".into(),
                    gates: vec![GateResult {
                        request: gate,
                        outcome: GateOutcome::Passed,
                        duration_ms: 4,
                    }],
                }),
            )
            .expect("commit verification");
        let Response::Mission(verified) = verified else {
            panic!("expected verified Mission");
        };
        assert_eq!(verified.mission.state.as_ref(), "integrating");
        let task = verified.tasks.first().expect("one verified Task");
        assert_eq!(task.state.as_ref(), "passed");
        assert!(task.receipt_id.is_some());
        let [artifact] = task.artifacts.as_slice() else {
            panic!("expected one Receipt Artifact");
        };
        assert_eq!(
            artifact.path.as_ref(),
            ".runtrol/handoffs/report/result.txt"
        );
        assert_eq!(artifact.size, 12);
        assert_eq!(artifact.sha256.as_ref(), hex(&[8; 32]));
        let integration = controller
            .integration_intent(&scratch.ledger, &verified.mission.mission_id, None)
            .expect("integration intent");
        let gates = integration
            .gate_requests
            .iter()
            .cloned()
            .map(|request| GateResult {
                request,
                outcome: GateOutcome::Passed,
                duration_ms: 3,
            })
            .collect();
        let completed = controller
            .commit_integration(
                &scratch.ledger,
                &integration,
                IntegrationEvidence {
                    artifacts: integration.expected_artifacts.clone(),
                    gates,
                },
            )
            .expect("complete integration");
        let Response::Mission(completed) = completed else {
            panic!("completed Mission");
        };
        assert_eq!(completed.mission.state.as_ref(), "completed");
        let archived = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionArchive {
                mission_id: completed.mission.mission_id.clone(),
            },
        );
        let Response::Mission(archived) = archived else {
            panic!("archived Mission");
        };
        assert_eq!(archived.mission.state.as_ref(), "archived");
    }

    #[test]
    fn integration_artifacts_are_rechecked_as_exact_receipt_evidence() {
        let expected = vec![ArtifactEvidence {
            path: "result.txt".into(),
            sha256: [7; 32],
            size: 12,
        }];
        let mut exact = expected.clone();
        assert!(integration_artifacts_match(&expected, &mut exact));
        let mut changed_by_gate = vec![ArtifactEvidence {
            path: "result.txt".into(),
            sha256: [8; 32],
            size: 12,
        }];
        assert!(!integration_artifacts_match(
            &expected,
            &mut changed_by_gate
        ));
    }

    #[test]
    fn reviewed_start_rechecks_mission_bytes_and_gate_policy() {
        let scratch = Scratch::make();
        scratch.write_mission();
        let mut controller = MissionController::default();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let Response::Mission(validated) = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated Mission");
        };
        let start = Request::MissionStart {
            mission_id: validated.mission.mission_id.clone(),
            mission_sha256: validated.mission_sha256.clone(),
        };
        std::fs::write(
            scratch.project.as_std_path().join("mission.toml"),
            b"changed\n",
        )
        .expect("change Mission");
        let Response::Failed(changed) = controller.answer(&scratch.ledger, &[], &[], &start) else {
            panic!("changed Mission refusal");
        };
        assert!(changed.message.contains("changed after review"));

        scratch.write_mission();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: vec!["changed".into()],
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let Response::Failed(changed) = controller.answer(&scratch.ledger, &[], &[], &start) else {
            panic!("changed policy refusal");
        };
        assert!(changed.message.contains("Gate policy changed"));
    }

    #[test]
    fn reviewed_start_refuses_an_expired_local_approval() {
        let scratch = Scratch::make();
        scratch.write_mission();
        let mut controller = MissionController::default();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let Response::Mission(validated) = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated Mission");
        };
        let mission_id: MissionId = validated
            .mission
            .mission_id
            .parse()
            .expect("Mission identity");
        let mut snapshot = scratch
            .ledger
            .snapshot(mission_id)
            .expect("read Mission")
            .expect("stored Mission");
        snapshot.mission.approval_expires_unix_ms = 1;
        scratch.ledger.put(&snapshot).expect("expire approval");

        let Response::Failed(expired) = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionStart {
                mission_id: validated.mission.mission_id.clone(),
                mission_sha256: validated.mission_sha256.clone(),
            },
        ) else {
            panic!("expired approval refusal");
        };
        assert!(expired.message.contains("approval expired"));
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "the comparison fixture proves worktree isolation and exact selected Receipt integration together"
    )]
    async fn parallel_writers_receive_distinct_linked_worktrees() {
        let scratch = Scratch::make();
        let containment = Containment::without_any();
        let git = resolve("git").expect("Git for product fixture");
        git_ok(
            &git,
            scratch.project.as_str(),
            &["init", "--initial-branch=main"],
            &containment,
        )
        .await;
        git_ok(
            &git,
            scratch.project.as_str(),
            &["config", "user.email", "fixture@runtrol.invalid"],
            &containment,
        )
        .await;
        git_ok(
            &git,
            scratch.project.as_str(),
            &["config", "user.name", "Runtrol Fixture"],
            &containment,
        )
        .await;
        let instruction = b"write only the declared comparison output\n";
        std::fs::write(
            scratch.project.as_std_path().join("instructions/task.md"),
            instruction,
        )
        .expect("instruction");
        let digest: [u8; 32] = Sha256::digest(instruction).into();
        let mission = format!(
            r#"schema = "runtrol.dev/mission/v1alpha1"
name = "parallel fixture"
project_id = "parallel-project"
base_ref = "main"
require_clean_base = true
completion_policy = "choose_one"

[limits]
max_parallel_tasks = 2
max_hot_providers = 2
max_runs_per_task = 1
max_repair_cycles = 0
stop_on_critical_failure = false

[[tasks]]
id = "branch-one"
instruction_ref = "instructions/task.md"
instruction_sha256 = "{}"
workspace_mode = "isolated_worktree"
provider_selector = "operator_choice"
output_roots = ["outputs/result.txt"]
gate_refs = ["fixture-check"]

[[tasks]]
id = "branch-two"
instruction_ref = "instructions/task.md"
instruction_sha256 = "{}"
workspace_mode = "isolated_worktree"
provider_selector = "operator_choice"
output_roots = ["outputs/result.txt"]
gate_refs = ["fixture-check"]
"#,
            hex(&digest),
            hex(&digest),
        );
        std::fs::write(scratch.project.as_std_path().join("mission.toml"), mission)
            .expect("Mission");
        git_ok(
            &git,
            scratch.project.as_str(),
            &["add", "--", "mission.toml", "instructions"],
            &containment,
        )
        .await;
        git_ok(
            &git,
            scratch.project.as_str(),
            &["commit", "-m", "fixture"],
            &containment,
        )
        .await;

        let mut controller = MissionController::default();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "git".into(),
                    arguments: vec!["status".into(), "--short".into()],
                    timeout_ms: 10_000,
                },
            ),
            Response::Done
        ));
        let Response::Mission(validated) = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated parallel Mission");
        };
        let mission_id = validated.mission.mission_id.clone();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionStart {
                    mission_id: mission_id.clone(),
                    mission_sha256: validated.mission_sha256.clone(),
                },
            ),
            Response::Mission(_)
        ));
        let mut prepared = Vec::new();
        for task in &validated.tasks {
            let WorkspacePreparation::Run(intent) = controller
                .workspace_intent(&scratch.ledger, &mission_id, &task.task_id)
                .expect("workspace intent")
            else {
                panic!("new worktree intent");
            };
            let (workspace, base_commit) = run_workspace_preparation(&intent, &containment)
                .await
                .expect("prepare linked worktree");
            controller
                .commit_workspace(&scratch.ledger, &intent, workspace.clone(), base_commit)
                .expect("commit workspace");
            prepared.push(workspace);
        }
        assert_eq!(prepared.len(), 2);
        assert_ne!(prepared.first(), prepared.get(1));
        let first = ProjectIdentity::discover(prepared.first().expect("first").clone())
            .expect("first identity");
        let second = ProjectIdentity::discover(prepared.get(1).expect("second").clone())
            .expect("second identity");
        assert_ne!(first.worktree(), second.worktree());
        assert_eq!(first.common_store(), second.common_store());
        let mut passing_digests = Vec::new();
        for (index, (task, workspace)) in validated.tasks.iter().zip(&prepared).enumerate() {
            let runtime_session = runtrol_provider::SessionId::now().to_string();
            assert!(matches!(
                controller.answer(
                    &scratch.ledger,
                    &[],
                    &[],
                    &Request::MissionBindSession {
                        mission_id: mission_id.clone(),
                        task_id: task.task_id.clone(),
                        session_id: runtime_session.into(),
                        provider_runtime_id: "fixture-provider".into(),
                        native_session_id: None,
                        workspace: workspace.as_str().into(),
                    },
                ),
                Response::Mission(_)
            ));
            let Response::MissionInstruction(sent) = controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionSendTaskInstruction {
                    mission_id: mission_id.clone(),
                    task_id: task.task_id.clone(),
                    instruction_sha256: hex(&digest).into(),
                },
            ) else {
                panic!("exact comparison instruction");
            };
            let intent = controller
                .verification_intent(
                    &scratch.ledger,
                    &sent.mission_id,
                    &sent.task_id,
                    &format!("native-{index}"),
                )
                .expect("verification intent");
            let gate = intent.gate_requests.first().expect("Gate request").clone();
            let artifact_digest = [u8::try_from(index + 1).expect("bounded fixture"); 32];
            passing_digests.push(artifact_digest);
            controller
                .commit_verification(
                    &scratch.ledger,
                    &intent,
                    Ok(VerificationEvidence {
                        binary_fingerprint: [9; 32],
                        artifacts: vec![ArtifactEvidence {
                            path: "outputs/result.txt".into(),
                            sha256: artifact_digest,
                            size: 12,
                        }],
                        finish_tree: format!("snapshot:{index}").into(),
                        gates: vec![GateResult {
                            request: gate,
                            outcome: GateOutcome::Passed,
                            duration_ms: 1,
                        }],
                    }),
                )
                .expect("passing comparison result");
        }
        let Response::Mission(snapshot) = controller.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionGet {
                mission_id: mission_id.clone(),
            },
        ) else {
            panic!("comparison snapshot");
        };
        assert_eq!(snapshot.mission.state.as_ref(), "integrating");
        assert_eq!(snapshot.mission.completion_policy.as_ref(), "chooseOne");
        assert!(
            controller
                .integration_intent(&scratch.ledger, &mission_id, None)
                .is_err()
        );
        let selected_task_id = validated
            .tasks
            .get(1)
            .expect("second validated task")
            .task_id
            .clone();
        let selected = controller
            .integration_intent(
                &scratch.ledger,
                &mission_id,
                Some(selected_task_id.as_ref()),
            )
            .expect("selected passing result");
        assert_eq!(selected.expected_artifacts.len(), 1);
        let selected_digest = selected
            .expected_artifacts
            .first()
            .expect("selected artifact")
            .sha256;
        assert_eq!(
            selected_digest,
            *passing_digests.get(1).expect("second passing digest")
        );
        assert_ne!(
            selected_digest,
            *passing_digests.first().expect("first passing digest")
        );
        let selected_receipt_id = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == selected_task_id)
            .and_then(|task| task.receipt_id.as_deref())
            .expect("selected passing Receipt")
            .to_owned();
        let selected_run_id = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == selected_task_id)
            .and_then(|task| task.run_id.as_deref())
            .expect("selected passing Run")
            .to_owned();
        assert!(!selected.gate_requests.is_empty());
        assert!(
            selected
                .gate_requests
                .iter()
                .all(|request| request.run_id.to_string() == selected_run_id)
        );
        let integration_gates = selected
            .gate_requests
            .iter()
            .cloned()
            .map(|request| GateResult {
                request,
                outcome: GateOutcome::Passed,
                duration_ms: 1,
            })
            .collect();
        let completed = controller
            .commit_integration(
                &scratch.ledger,
                &selected,
                IntegrationEvidence {
                    artifacts: selected.expected_artifacts.clone(),
                    gates: integration_gates,
                },
            )
            .expect("complete selected comparison result");
        let Response::Mission(completed) = completed else {
            panic!("completed comparison Mission");
        };
        let completion = completed
            .integration
            .expect("durable integration authority");
        assert_eq!(
            completion.selected_task_id.as_deref(),
            Some(selected_task_id.as_ref())
        );
        assert_eq!(
            completion.selected_receipt_id.as_deref(),
            Some(selected_receipt_id.as_str())
        );
        let durable = scratch
            .ledger
            .snapshot(mission_id.parse().expect("Mission identity"))
            .expect("read completed Mission")
            .expect("completed Mission exists");
        let durable_completion = durable
            .mission
            .integration
            .expect("completion authority survives compaction");
        assert_eq!(
            durable_completion
                .selected_task_id
                .map(|task| task.to_string()),
            Some(selected_task_id.to_string())
        );
        assert_eq!(
            durable_completion
                .selected_receipt_id
                .map(|receipt| receipt.to_string()),
            Some(selected_receipt_id)
        );

        for workspace in prepared {
            git_ok(
                &git,
                scratch.project.as_str(),
                &["worktree", "remove", "--force", workspace.as_str()],
                &containment,
            )
            .await;
        }
    }

    async fn git_ok(
        git: &runtrol_childproc::Program,
        project: &str,
        arguments: &[&str],
        containment: &Containment,
    ) {
        let mut argv = vec!["-C".to_owned(), project.to_owned()];
        argv.extend(arguments.iter().map(ToString::to_string));
        let output = capture(git, &argv, Duration::from_secs(20), containment)
            .await
            .expect("Git fixture command");
        assert!(output.succeeded(), "Git fixture command failed");
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one restart test keeps schedule persistence, claim, replay, and finish assertions together"
    )]
    fn future_schedule_survives_restart_and_has_one_durable_due_claim() {
        let scratch = Scratch::make();
        scratch.write_mission();
        let gate_path = scratch
            .project
            .join("scheduled-gates.json")
            .expect("gate path");
        let mut before = MissionController::open(gate_path.clone()).expect("open gates");
        assert!(matches!(
            before.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let runtime_ids = vec!["runtime-fixture".into()];
        let Response::Mission(validated) = before.answer(
            &scratch.ledger,
            &runtime_ids,
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated Mission");
        };
        let mission_id = validated.mission.mission_id.clone();
        let task_id = validated.tasks.first().expect("Task").task_id.clone();
        let schedule_id: Box<str> = ScheduleId::now().to_string().into();
        let due_unix_ms = current_unix_ms().expect("clock") + 5_000;
        let request = Request::MissionSchedule {
            schedule_id: schedule_id.clone(),
            replaces_schedule_id: None,
            mission_id: mission_id.clone(),
            mission_sha256: validated.mission_sha256.clone(),
            due_unix_ms,
            providers: vec![MissionScheduleProviderLine {
                task_id: task_id.clone(),
                provider_runtime_id: "runtime-fixture".into(),
            }],
        };
        let Response::Mission(scheduled) =
            before.answer(&scratch.ledger, &runtime_ids, &[], &request)
        else {
            panic!("scheduled Mission");
        };
        assert_eq!(
            scheduled
                .mission
                .schedule
                .as_ref()
                .expect("schedule")
                .state
                .as_ref(),
            "pending"
        );
        assert!(matches!(
            before.answer(&scratch.ledger, &runtime_ids, &[], &request),
            Response::Mission(_)
        ));
        drop(before);

        let mut after = MissionController::open(gate_path).expect("restore gates");
        let trust_path = scratch
            .project
            .join("scheduled-capability-trust.json")
            .expect("trust path");
        let mut growth = crate::growth::GrowthController::open(trust_path).expect("growth");
        after
            .recover(&scratch.ledger, &runtime_ids, &mut growth)
            .expect("recover scheduled Mission");
        assert_eq!(
            MissionController::next_schedule_wake(&scratch.ledger).expect("next due"),
            Some(due_unix_ms)
        );
        assert!(
            after
                .claim_due_schedule(&scratch.ledger, &mut growth, due_unix_ms - 1)
                .expect("early claim")
                .is_none()
        );
        let execution = after
            .claim_due_schedule(&scratch.ledger, &mut growth, due_unix_ms)
            .expect("due claim")
            .expect("due execution");
        assert_eq!(execution.mission_id, mission_id);
        assert_eq!(execution.schedule_id, schedule_id);
        assert_eq!(
            execution.providers.get(&task_id).map(Box::as_ref),
            Some("runtime-fixture")
        );
        let repeated = after
            .claim_due_schedule(&scratch.ledger, &mut growth, due_unix_ms + 1)
            .expect("repeated claim")
            .expect("same launching execution");
        assert_eq!(repeated.schedule_id, execution.schedule_id);
        MissionController::finish_schedule_launch(
            &scratch.ledger,
            &execution.mission_id,
            &execution.schedule_id,
            None,
        )
        .expect("finish launch");
        assert_eq!(
            MissionController::next_schedule_wake(&scratch.ledger).expect("no wake"),
            None
        );
        let snapshot = scratch
            .ledger
            .snapshot(mission_id.parse().expect("Mission ID"))
            .expect("ledger")
            .expect("snapshot");
        assert_eq!(snapshot.mission.state, MissionState::Running);
        assert_eq!(
            snapshot.mission.schedule.expect("schedule").state,
            MissionScheduleState::Started
        );
        assert_eq!(
            snapshot.tasks.first().expect("Task").state,
            TaskState::Reserved
        );
    }

    #[test]
    fn schedule_cancel_is_exact_and_prevents_a_due_claim() {
        let scratch = Scratch::make();
        scratch.write_mission();
        let mut controller = MissionController::default();
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let runtime_ids = vec!["runtime-fixture".into()];
        let Response::Mission(validated) = controller.answer(
            &scratch.ledger,
            &runtime_ids,
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated Mission");
        };
        let task_id = validated.tasks.first().expect("Task").task_id.clone();
        let schedule_id: Box<str> = ScheduleId::now().to_string().into();
        let due_unix_ms = current_unix_ms().expect("clock") + 5_000;
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &runtime_ids,
                &[],
                &Request::MissionSchedule {
                    schedule_id: schedule_id.clone(),
                    replaces_schedule_id: None,
                    mission_id: validated.mission.mission_id.clone(),
                    mission_sha256: validated.mission_sha256.clone(),
                    due_unix_ms,
                    providers: vec![MissionScheduleProviderLine {
                        task_id,
                        provider_runtime_id: "runtime-fixture".into(),
                    }],
                },
            ),
            Response::Mission(_)
        ));
        assert!(matches!(
            controller.answer(
                &scratch.ledger,
                &runtime_ids,
                &[],
                &Request::MissionScheduleCancel {
                    mission_id: validated.mission.mission_id.clone(),
                    mission_sha256: validated.mission_sha256,
                    schedule_id: schedule_id.clone(),
                },
            ),
            Response::Mission(_)
        ));
        assert_eq!(
            MissionController::next_schedule_wake(&scratch.ledger).expect("wake"),
            None
        );
        let snapshot = scratch
            .ledger
            .snapshot(validated.mission.mission_id.parse().expect("Mission ID"))
            .expect("ledger")
            .expect("snapshot");
        let schedule = snapshot.mission.schedule.expect("schedule");
        assert_eq!(schedule.id.to_string(), schedule_id.as_ref());
        assert_eq!(schedule.state, MissionScheduleState::Cancelled);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the restart fixture crosses every durable boundary without helper-hidden state"
    )]
    fn restart_restores_gates_and_blocks_unsent_session_without_resubmission() {
        let scratch = Scratch::make();
        let instruction_digest = scratch.write_mission();
        let gate_path = scratch
            .project
            .join("mission-gates.json")
            .expect("gate path");
        let mut before = MissionController::open(gate_path.clone()).expect("open gates");
        assert!(matches!(
            before.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionRegisterGate {
                    gate_id: "fixture-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    timeout_ms: 1_000,
                },
            ),
            Response::Done
        ));
        let Response::Mission(validated) = before.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionValidate {
                project: scratch.project.as_str().into(),
                mission_ref: "mission.toml".into(),
            },
        ) else {
            panic!("validated Mission");
        };
        let mission_id = validated.mission.mission_id.clone();
        let task_id = validated.tasks.first().expect("Task").task_id.clone();
        assert!(matches!(
            before.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionStart {
                    mission_id: mission_id.clone(),
                    mission_sha256: validated.mission_sha256.clone(),
                },
            ),
            Response::Mission(_)
        ));
        let WorkspacePreparation::Run(intent) = before
            .workspace_intent(&scratch.ledger, &mission_id, &task_id)
            .expect("workspace intent")
        else {
            panic!("workspace preparation");
        };
        before
            .commit_workspace(
                &scratch.ledger,
                &intent,
                scratch.project.clone(),
                "fixture-base".into(),
            )
            .expect("workspace");
        assert!(matches!(
            before.answer(
                &scratch.ledger,
                &[],
                &[],
                &Request::MissionBindSession {
                    mission_id: mission_id.clone(),
                    task_id: task_id.clone(),
                    session_id: runtrol_provider::SessionId::now().to_string().into(),
                    provider_runtime_id: "fixture-provider".into(),
                    native_session_id: None,
                    workspace: scratch.project.as_str().into(),
                },
            ),
            Response::Mission(_)
        ));
        drop(before);

        let mut after = MissionController::open(gate_path).expect("restore gates");
        let trust_path = scratch
            .project
            .join("capability-trust.json")
            .expect("trust path");
        let mut growth = crate::growth::GrowthController::open(trust_path).expect("growth");
        after
            .recover(&scratch.ledger, &[], &mut growth)
            .expect("recover Mission");
        let Response::Mission(recovered) = after.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionGet {
                mission_id: mission_id.clone(),
            },
        ) else {
            panic!("recovered Mission");
        };
        assert_eq!(recovered.mission.state.as_ref(), "blocked");
        let task = recovered.tasks.first().expect("recovered Task");
        assert_eq!(task.state.as_ref(), "blocked");
        assert!(task.session_id.is_none());
        assert_eq!(task.instruction_sha256.as_ref(), hex(&instruction_digest));
        let snapshot = scratch
            .ledger
            .snapshot(mission_id.parse().expect("Mission ID"))
            .expect("ledger read")
            .expect("snapshot");
        assert!(snapshot.runs.is_empty(), "restart must not invent a Send");

        let Response::Mission(reopened) = after.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionRetryTask {
                mission_id: mission_id.clone(),
                task_id,
            },
        ) else {
            panic!("reopened recovery Task");
        };
        assert_eq!(reopened.mission.state.as_ref(), "blocked");
        assert_eq!(
            reopened
                .tasks
                .first()
                .expect("reopened Task")
                .state
                .as_ref(),
            "eligible"
        );
        let Response::Mission(resumed) = after.answer(
            &scratch.ledger,
            &[],
            &[],
            &Request::MissionResumeSafe { mission_id },
        ) else {
            panic!("safely resumed Mission");
        };
        assert_eq!(resumed.mission.state.as_ref(), "running");
        let resumed_task = resumed.tasks.first().expect("resumed Task");
        assert_eq!(resumed_task.state.as_ref(), "reserved");
        assert!(
            resumed_task.session_id.is_none(),
            "recovery must require one fresh Runtime session"
        );
    }
}
