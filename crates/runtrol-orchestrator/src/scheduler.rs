//! Deterministic bounded reservations and provider-neutral effects.

use std::collections::{BTreeMap, BTreeSet};

use runtrol_ledger::{RunId, TaskId};

use crate::{
    CompletionPolicy, InstructionRef, ProviderSelector, ValidatedMission, ValidatedTask,
    WorkspaceMode,
};

/// Global scheduler resource ceiling supplied by daemon composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Maximum simultaneous provider processes across Missions.
    pub max_hot_providers: u8,
}

/// Current eligibility of one Task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    /// At least one dependency has not passed.
    Waiting,
    /// Every dependency passed and reservation is allowed.
    Ready,
    /// The Task already owns a reservation.
    Reserved,
    /// The Task passed and releases dependent work.
    Passed,
    /// The Task cannot continue.
    Terminal,
}

/// Content-free Task fact used to rebuild scheduler admission after daemon restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryTaskState {
    /// Durable passing evidence exists.
    Passed,
    /// The Task may be admitted after dependency and pause checks.
    Open,
    /// The Task cannot execute again.
    Terminal,
}

/// Atomic reservation of all resources required before session preparation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    /// New Run identity committed before effects.
    pub run_id: RunId,
    /// Exact generated Task identity.
    pub task_id: TaskId,
    /// Stable Task key.
    pub task_key: Box<str>,
    /// Workspace collision posture.
    pub workspace_mode: WorkspaceMode,
    /// Resolved provider selection posture.
    pub provider_selector: ProviderSelector,
    /// Exact output claims held by this reservation.
    pub output_roots: Vec<Box<str>>,
    /// Reviewed instruction identity, never its bytes.
    pub instruction: InstructionRef,
}

/// Effect intent consumed only by daemon composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerEffect {
    /// Open or resume an exact provider-native session without submitting input.
    PrepareSession(Reservation),
    /// Run one exact fixed deterministic gate.
    RunGate {
        /// Exact Run being verified.
        run_id: RunId,
        /// Exact registry identity.
        gate_id: Box<str>,
    },
    /// Present passed branches for local integration review.
    PresentIntegration,
    /// Finish a comparison Mission because every attempt ended without a passing result.
    FinishWithoutPassingResult,
    /// Release only the proven reservation identity.
    ReleaseResources {
        /// Run whose resources are released.
        run_id: RunId,
    },
}

/// Local-only authorization to read and transport exact reviewed bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalInstructionSubmission {
    /// Exact reserved Run.
    pub run_id: RunId,
    /// Exact Task.
    pub task_id: TaskId,
    /// Reviewed project file and digest.
    pub instruction: InstructionRef,
}

/// One bounded deterministic Mission scheduler.
#[derive(Clone, Debug)]
pub struct Scheduler {
    mission: ValidatedMission,
    budget: ResourceBudget,
    states: BTreeMap<Box<str>, Eligibility>,
    reservations: BTreeMap<Box<str>, Reservation>,
    paused: bool,
    cancelled: bool,
}

impl Scheduler {
    /// Create one scheduler with every root Task ready and every dependent Task waiting.
    ///
    /// # Errors
    /// Returns [`SchedulerError::InvalidBudget`] when the global budget is zero.
    pub fn new(mission: ValidatedMission, budget: ResourceBudget) -> Result<Self, SchedulerError> {
        if budget.max_hot_providers == 0 {
            return Err(SchedulerError::InvalidBudget);
        }
        let states = mission
            .tasks
            .iter()
            .map(|task| {
                (
                    task.key.clone(),
                    if task.depends_on.is_empty() {
                        Eligibility::Ready
                    } else {
                        Eligibility::Waiting
                    },
                )
            })
            .collect();
        Ok(Self {
            mission,
            budget,
            states,
            reservations: BTreeMap::new(),
            paused: false,
            cancelled: false,
        })
    }

    /// Rebuild admission from durable Task facts without recreating an active Run or submitting input.
    ///
    /// # Errors
    /// Returns [`SchedulerError::InvalidBudget`] for a zero global budget or
    /// [`SchedulerError::RecoveryMismatch`] when a durable Task identity is missing.
    pub fn recover(
        mission: ValidatedMission,
        budget: ResourceBudget,
        recovered: &BTreeMap<TaskId, RecoveryTaskState>,
        paused: bool,
    ) -> Result<Self, SchedulerError> {
        if budget.max_hot_providers == 0 {
            return Err(SchedulerError::InvalidBudget);
        }
        if mission
            .tasks
            .iter()
            .any(|task| !recovered.contains_key(&task.id))
            || recovered.len() != mission.tasks.len()
        {
            return Err(SchedulerError::RecoveryMismatch);
        }
        let completed: BTreeSet<&str> = mission
            .tasks
            .iter()
            .filter(|task| recovered.get(&task.id) == Some(&RecoveryTaskState::Passed))
            .map(|task| task.key.as_ref())
            .collect();
        let states = mission
            .tasks
            .iter()
            .map(|task| {
                let state = match recovered.get(&task.id) {
                    Some(RecoveryTaskState::Passed) => Eligibility::Passed,
                    Some(RecoveryTaskState::Terminal) | None => Eligibility::Terminal,
                    Some(RecoveryTaskState::Open)
                        if task
                            .depends_on
                            .iter()
                            .all(|dependency| completed.contains(dependency.as_ref())) =>
                    {
                        Eligibility::Ready
                    }
                    Some(RecoveryTaskState::Open) => Eligibility::Waiting,
                };
                (task.key.clone(), state)
            })
            .collect();
        Ok(Self {
            mission,
            budget,
            states,
            reservations: BTreeMap::new(),
            paused,
            cancelled: false,
        })
    }

    /// Reserve the first stable ready Task or report why no reservation exists.
    ///
    /// # Errors
    /// Returns a typed scheduler refusal for pause, cancellation, exhausted slots, or overlapping claims.
    pub fn reserve_next(&mut self) -> Result<SchedulerEffect, SchedulerError> {
        if self.cancelled {
            return Err(SchedulerError::Cancelled);
        }
        if self.paused {
            return Err(SchedulerError::Paused);
        }
        let mission_limit = usize::from(self.mission.spec.limits.max_parallel_tasks);
        let global_limit = usize::from(self.budget.max_hot_providers);
        if self.reservations.len() >= mission_limit.min(global_limit) {
            return Err(SchedulerError::SlotsExhausted);
        }
        let task = self
            .mission
            .tasks
            .iter()
            .find(|task| self.states.get(&task.key) == Some(&Eligibility::Ready))
            .cloned()
            .ok_or(SchedulerError::NothingReady)?;
        if self.mission.spec.completion_policy == CompletionPolicy::AllTasks
            && self
                .reservations
                .values()
                .any(|active| claims_overlap(&active.output_roots, &task.output_roots))
        {
            return Err(SchedulerError::OutputClaim);
        }
        let reservation = reservation_of(&task);
        self.states.insert(task.key.clone(), Eligibility::Reserved);
        self.reservations.insert(task.key, reservation.clone());
        Ok(SchedulerEffect::PrepareSession(reservation))
    }

    /// Mark the prepared Task as waiting for a distinct local Send action.
    ///
    /// # Errors
    /// Returns [`SchedulerError::NotReserved`] when the exact Task has no active reservation.
    pub fn local_submission(
        &self,
        task_id: TaskId,
    ) -> Result<LocalInstructionSubmission, SchedulerError> {
        let reservation = self
            .reservations
            .values()
            .find(|reservation| reservation.task_id == task_id)
            .ok_or(SchedulerError::NotReserved)?;
        Ok(LocalInstructionSubmission {
            run_id: reservation.run_id,
            task_id,
            instruction: reservation.instruction.clone(),
        })
    }

    /// Seal one Task as passed, release its resources, and make exact dependents ready.
    ///
    /// # Errors
    /// Returns [`SchedulerError::NotReserved`] when the exact Run does not own a reservation.
    pub fn pass(&mut self, run_id: RunId) -> Result<Vec<SchedulerEffect>, SchedulerError> {
        let key = self
            .reservations
            .iter()
            .find_map(|(key, reservation)| (reservation.run_id == run_id).then(|| key.clone()))
            .ok_or(SchedulerError::NotReserved)?;
        self.reservations.remove(&key);
        self.states.insert(key.clone(), Eligibility::Passed);
        let passed: BTreeSet<Box<str>> = self
            .states
            .iter()
            .filter(|(_, state)| **state == Eligibility::Passed)
            .map(|(task, _)| task.clone())
            .collect();
        for task in &self.mission.tasks {
            if self.states.get(&task.key) == Some(&Eligibility::Waiting)
                && task
                    .depends_on
                    .iter()
                    .all(|dependency| passed.contains(dependency.as_ref()))
            {
                self.states.insert(task.key.clone(), Eligibility::Ready);
            }
        }
        let mut effects = vec![SchedulerEffect::ReleaseResources { run_id }];
        if let Some(effect) = self.completion_effect() {
            effects.push(effect);
        }
        Ok(effects)
    }

    /// Release one failed Run and return its exact Task to eligibility for a locally approved retry.
    ///
    /// # Errors
    /// Returns [`SchedulerError::NotReserved`] when the exact Run owns no reservation.
    pub fn retry(&mut self, run_id: RunId) -> Result<SchedulerEffect, SchedulerError> {
        let key = self
            .reservations
            .iter()
            .find_map(|(key, reservation)| (reservation.run_id == run_id).then(|| key.clone()))
            .ok_or(SchedulerError::NotReserved)?;
        self.reservations.remove(&key);
        self.states.insert(key, Eligibility::Ready);
        Ok(SchedulerEffect::ReleaseResources { run_id })
    }

    /// Release one terminal failed Run without making its Task eligible again.
    ///
    /// # Errors
    /// Returns [`SchedulerError::NotReserved`] when the exact Run owns no reservation.
    pub fn fail(&mut self, run_id: RunId) -> Result<Vec<SchedulerEffect>, SchedulerError> {
        let key = self
            .reservations
            .iter()
            .find_map(|(key, reservation)| (reservation.run_id == run_id).then(|| key.clone()))
            .ok_or(SchedulerError::NotReserved)?;
        self.reservations.remove(&key);
        self.states.insert(key, Eligibility::Terminal);
        let mut effects = vec![SchedulerEffect::ReleaseResources { run_id }];
        if let Some(effect) = self.completion_effect() {
            effects.push(effect);
        }
        Ok(effects)
    }

    /// Reopen one locally reviewed blocked or retryable Task after recovery.
    ///
    /// # Errors
    /// Returns [`SchedulerError::NotReserved`] when the Task is unknown or dependencies are not passed.
    pub fn reopen(&mut self, task_id: TaskId) -> Result<(), SchedulerError> {
        let task = self
            .mission
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or(SchedulerError::NotReserved)?;
        let ready = task
            .depends_on
            .iter()
            .all(|dependency| self.states.get(dependency.as_ref()) == Some(&Eligibility::Passed));
        if !ready {
            return Err(SchedulerError::NothingReady);
        }
        self.states.insert(task.key.clone(), Eligibility::Ready);
        Ok(())
    }

    /// Pause all new reservation admission.
    pub const fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume admission without changing contract or scopes.
    pub const fn resume_safe(&mut self) {
        self.paused = false;
    }

    /// Cancel new admission and return exact active Run release effects.
    pub fn cancel(&mut self) -> Vec<SchedulerEffect> {
        self.cancelled = true;
        let effects = self
            .reservations
            .values()
            .map(|reservation| SchedulerEffect::ReleaseResources {
                run_id: reservation.run_id,
            })
            .collect();
        self.reservations.clear();
        for state in self.states.values_mut() {
            if *state != Eligibility::Passed {
                *state = Eligibility::Terminal;
            }
        }
        effects
    }

    /// Current state of one stable Task key.
    #[must_use]
    pub fn eligibility(&self, key: &str) -> Option<Eligibility> {
        self.states.get(key).copied()
    }

    fn completion_effect(&self) -> Option<SchedulerEffect> {
        match self.mission.spec.completion_policy {
            CompletionPolicy::AllTasks => self
                .states
                .values()
                .all(|state| *state == Eligibility::Passed)
                .then_some(SchedulerEffect::PresentIntegration),
            CompletionPolicy::ChooseOne => {
                let finished = self
                    .states
                    .values()
                    .all(|state| matches!(state, Eligibility::Passed | Eligibility::Terminal));
                if !finished {
                    None
                } else if self
                    .states
                    .values()
                    .any(|state| *state == Eligibility::Passed)
                {
                    Some(SchedulerEffect::PresentIntegration)
                } else {
                    Some(SchedulerEffect::FinishWithoutPassingResult)
                }
            }
        }
    }
}

fn reservation_of(task: &ValidatedTask) -> Reservation {
    Reservation {
        run_id: RunId::now(),
        task_id: task.id,
        task_key: task.key.clone(),
        workspace_mode: task.workspace_mode,
        provider_selector: task.provider_selector.clone(),
        output_roots: task.output_roots.clone(),
        instruction: task.instruction.clone(),
    }
}

fn claims_overlap(left: &[Box<str>], right: &[Box<str>]) -> bool {
    left.iter().any(|left| {
        right.iter().any(|right| {
            left == right
                || left
                    .strip_prefix(right.as_ref())
                    .is_some_and(|suffix| suffix.starts_with('/'))
                || right
                    .strip_prefix(left.as_ref())
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    })
}

/// Deterministic scheduler refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchedulerError {
    /// Global hot process budget is zero.
    #[error("scheduler resource budget is invalid")]
    InvalidBudget,
    /// Mission admission is paused.
    #[error("mission admission is paused")]
    Paused,
    /// Mission was cancelled.
    #[error("mission was cancelled")]
    Cancelled,
    /// Parallel or global hot slot ceiling was reached.
    #[error("scheduler slots are exhausted")]
    SlotsExhausted,
    /// No dependency-complete Task exists.
    #[error("no task is ready")]
    NothingReady,
    /// Output claim overlaps an active reservation.
    #[error("task output claim overlaps an active reservation")]
    OutputClaim,
    /// Exact Task or Run has no active reservation.
    #[error("task or run is not reserved")]
    NotReserved,
    /// Durable Task identities did not exactly match the reviewed Mission.
    #[error("durable task identities do not match the reviewed mission")]
    RecoveryMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompletionPolicy, MissionLimits, MissionSpec};
    use runtrol_core::ProjectIdentity;
    use runtrol_provider::AbsPath;

    struct Scratch {
        root: std::path::PathBuf,
        canonical: AbsPath,
    }

    impl Scratch {
        fn make() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-scheduler-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear scratch");
            }
            std::fs::create_dir_all(&root).expect("create scratch");
            let canonical =
                AbsPath::canonicalize(root.to_str().expect("UTF-8")).expect("canonical");
            Self { root, canonical }
        }

        fn mission(&self) -> ValidatedMission {
            let first = ValidatedTask {
                id: TaskId::now(),
                key: "first".into(),
                depends_on: Vec::new(),
                instruction: InstructionRef {
                    path: "first.md".into(),
                    sha256: [1; 32],
                },
                workspace_mode: WorkspaceMode::IsolatedWorktree,
                provider_selector: ProviderSelector::OperatorChoice,
                output_roots: vec!["src".into()],
                gate_refs: vec!["check".into()],
                capability_versions: Vec::new(),
            };
            let second = ValidatedTask {
                id: TaskId::now(),
                key: "second".into(),
                depends_on: vec!["first".into()],
                instruction: InstructionRef {
                    path: "second.md".into(),
                    sha256: [2; 32],
                },
                workspace_mode: WorkspaceMode::IsolatedWorktree,
                provider_selector: ProviderSelector::Exact("runtime".into()),
                output_roots: vec!["tests".into()],
                gate_refs: vec!["check".into()],
                capability_versions: Vec::new(),
            };
            ValidatedMission {
                mission_sha256: [7; 32],
                project: ProjectIdentity::discover(self.canonical.clone()).expect("project"),
                spec: MissionSpec {
                    schema: crate::MISSION_SCHEMA.into(),
                    name: "fixture".into(),
                    project_id: "project".into(),
                    base_ref: "main".into(),
                    require_clean_base: true,
                    completion_policy: CompletionPolicy::AllTasks,
                    limits: MissionLimits {
                        max_parallel_tasks: 2,
                        max_hot_providers: 2,
                        max_runs_per_task: 2,
                        max_repair_cycles: 1,
                        stop_on_critical_failure: true,
                    },
                    tasks: Vec::new(),
                },
                tasks: vec![first, second],
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn scheduler_prepares_without_input_and_releases_dependencies_in_order() {
        let scratch = Scratch::make();
        let mut scheduler = Scheduler::new(
            scratch.mission(),
            ResourceBudget {
                max_hot_providers: 2,
            },
        )
        .expect("scheduler");
        let SchedulerEffect::PrepareSession(first) =
            scheduler.reserve_next().expect("first reservation")
        else {
            panic!("expected preparation");
        };
        let local = scheduler
            .local_submission(first.task_id)
            .expect("local send");
        assert_eq!(local.instruction.sha256, [1; 32]);
        scheduler.pass(first.run_id).expect("pass first");
        assert_eq!(scheduler.eligibility("second"), Some(Eligibility::Ready));
    }

    #[test]
    fn pause_cancel_and_slot_limits_are_explicit() {
        let scratch = Scratch::make();
        let mut scheduler = Scheduler::new(
            scratch.mission(),
            ResourceBudget {
                max_hot_providers: 1,
            },
        )
        .expect("scheduler");
        scheduler.pause();
        assert_eq!(scheduler.reserve_next(), Err(SchedulerError::Paused));
        scheduler.resume_safe();
        assert!(scheduler.reserve_next().is_ok());
        assert_eq!(
            scheduler.reserve_next(),
            Err(SchedulerError::SlotsExhausted)
        );
        assert_eq!(scheduler.cancel().len(), 1);
        assert_eq!(scheduler.reserve_next(), Err(SchedulerError::Cancelled));
    }

    #[test]
    fn choose_one_runs_overlapping_attempts_and_finishes_after_all_results() {
        let scratch = Scratch::make();
        let mut mission = scratch.mission();
        mission.spec.completion_policy = CompletionPolicy::ChooseOne;
        mission.spec.limits.max_runs_per_task = 1;
        mission.spec.limits.max_repair_cycles = 0;
        mission.spec.limits.stop_on_critical_failure = false;
        let first_instruction = mission
            .tasks
            .first()
            .expect("first task")
            .instruction
            .clone();
        let first_output_roots = mission
            .tasks
            .first()
            .expect("first task")
            .output_roots
            .clone();
        let second_task = mission.tasks.get_mut(1).expect("second task");
        second_task.depends_on.clear();
        second_task.instruction = first_instruction;
        second_task.output_roots = first_output_roots;
        let mut scheduler = Scheduler::new(
            mission,
            ResourceBudget {
                max_hot_providers: 2,
            },
        )
        .expect("scheduler");
        let SchedulerEffect::PrepareSession(first) = scheduler.reserve_next().expect("first")
        else {
            panic!("expected first preparation");
        };
        let SchedulerEffect::PrepareSession(second) = scheduler.reserve_next().expect("second")
        else {
            panic!("expected second preparation");
        };
        assert_eq!(
            scheduler.pass(first.run_id).expect("pass"),
            vec![SchedulerEffect::ReleaseResources {
                run_id: first.run_id
            }]
        );
        assert_eq!(
            scheduler.fail(second.run_id).expect("fail"),
            vec![
                SchedulerEffect::ReleaseResources {
                    run_id: second.run_id
                },
                SchedulerEffect::PresentIntegration,
            ]
        );
    }

    #[test]
    fn choose_one_reports_when_every_attempt_fails() {
        let scratch = Scratch::make();
        let mut mission = scratch.mission();
        mission.spec.completion_policy = CompletionPolicy::ChooseOne;
        let first_output_roots = mission
            .tasks
            .first()
            .expect("first task")
            .output_roots
            .clone();
        let second_task = mission.tasks.get_mut(1).expect("second task");
        second_task.depends_on.clear();
        second_task.output_roots = first_output_roots;
        let mut scheduler = Scheduler::new(
            mission,
            ResourceBudget {
                max_hot_providers: 2,
            },
        )
        .expect("scheduler");
        let SchedulerEffect::PrepareSession(first) = scheduler.reserve_next().expect("first")
        else {
            panic!("expected first preparation");
        };
        let SchedulerEffect::PrepareSession(second) = scheduler.reserve_next().expect("second")
        else {
            panic!("expected second preparation");
        };
        assert_eq!(
            scheduler.fail(first.run_id).expect("first failure").len(),
            1
        );
        assert_eq!(
            scheduler.fail(second.run_id).expect("second failure"),
            vec![
                SchedulerEffect::ReleaseResources {
                    run_id: second.run_id
                },
                SchedulerEffect::FinishWithoutPassingResult,
            ]
        );
    }
}
