//! Durable Mission schedule authority and due-claim transitions.

use super::*;

const MIN_SCHEDULE_LEAD_MS: u64 = 1_000;
const MAX_SCHEDULE_LEAD_MS: u64 = 366 * 24 * 60 * 60 * 1_000;

/// Exact durable authority handed to the Core-owned scheduled first-wave runner.
#[derive(Clone, Debug)]
pub(crate) struct MissionScheduleExecution {
    pub(crate) mission_id: Box<str>,
    pub(crate) schedule_id: Box<str>,
    pub(crate) providers: BTreeMap<Box<str>, Box<str>>,
}

impl MissionController {
    #[expect(
        clippy::too_many_arguments,
        reason = "one durable schedule binds every exact reviewed authority before it can outlive Studio"
    )]
    pub(super) fn schedule(
        &self,
        ledger: &Ledger,
        runtime_ids: &[Box<str>],
        schedule_id: &str,
        replaces_schedule_id: Option<&str>,
        mission_id: &str,
        mission_sha256: &str,
        due_unix_ms: u64,
        providers: &[MissionScheduleProviderLine],
    ) -> Result<Response, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let schedule_id: ScheduleId = schedule_id
            .parse()
            .map_err(|_| "the schedule identity is invalid")?;
        let replaces_schedule_id = replaces_schedule_id
            .map(str::parse::<ScheduleId>)
            .transpose()
            .map_err(|_| "the replaced schedule identity is invalid")?;
        let active = self
            .active
            .get(&mission_id)
            .ok_or("the Mission must be revalidated after restart")?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if snapshot.mission.state != MissionState::Validated {
            return Err("only a validated Mission can be scheduled");
        }
        let mission_digest = parse_digest(mission_sha256)?;
        if mission_digest != snapshot.mission.mission_sha256
            || mission_digest != active.validated.mission_sha256
        {
            return Err("the reviewed Mission digest changed");
        }
        let now = current_unix_ms()?;
        let minimum = now
            .checked_add(MIN_SCHEDULE_LEAD_MS)
            .ok_or("the schedule time is out of range")?;
        let maximum = now
            .checked_add(MAX_SCHEDULE_LEAD_MS)
            .ok_or("the schedule time is out of range")?;
        if !(minimum..=maximum).contains(&due_unix_ms) {
            return Err("the schedule must be between one second and 366 days from now");
        }
        let provider_map = reviewed_schedule_providers(active, runtime_ids, providers)?;
        let requested = MissionSchedule {
            id: schedule_id,
            due_unix_ms,
            mission_sha256: snapshot.mission.mission_sha256,
            policy_sha256: snapshot.mission.policy_sha256,
            providers: provider_map
                .into_iter()
                .map(|(task_id, provider_runtime_id)| MissionScheduleProvider {
                    task_id,
                    provider_runtime_id,
                })
                .collect(),
            state: MissionScheduleState::Pending,
            claimed_unix_ms: None,
            failure: None,
        };
        if let Some(existing) = &snapshot.mission.schedule
            && existing.id == schedule_id
        {
            if same_schedule_authority(existing, &requested) {
                return Ok(Self::snapshot_response(
                    &snapshot,
                    self.active.get(&mission_id),
                ));
            }
            return Err("the schedule identity was reused with different authority");
        }
        let current_pending = snapshot
            .mission
            .schedule
            .as_ref()
            .filter(|schedule| schedule.state == MissionScheduleState::Pending)
            .map(|schedule| schedule.id);
        if current_pending != replaces_schedule_id {
            return Err("the pending Mission schedule changed after review");
        }
        snapshot.mission.schedule = Some(requested);
        ledger
            .put(&snapshot)
            .map_err(|_| "the Mission schedule could not be committed")?;
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&mission_id),
        ))
    }

    pub(super) fn cancel_schedule(
        &self,
        ledger: &Ledger,
        mission_id: &str,
        mission_sha256: &str,
        schedule_id: &str,
    ) -> Result<Response, &'static str> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid")?;
        let schedule_id: ScheduleId = schedule_id
            .parse()
            .map_err(|_| "the schedule identity is invalid")?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|_| "the Mission ledger cannot be read")?
            .ok_or("the Mission does not exist")?;
        if parse_digest(mission_sha256)? != snapshot.mission.mission_sha256 {
            return Err("the reviewed Mission digest changed");
        }
        let schedule = snapshot
            .mission
            .schedule
            .as_mut()
            .ok_or("the Mission has no schedule")?;
        if schedule.id != schedule_id {
            return Err("the current schedule identity changed");
        }
        match schedule.state {
            MissionScheduleState::Pending => {
                schedule.state = MissionScheduleState::Cancelled;
                schedule.failure = None;
                ledger
                    .put(&snapshot)
                    .map_err(|_| "the Mission schedule cancellation could not be committed")?;
            }
            MissionScheduleState::Cancelled => {}
            MissionScheduleState::Launching
            | MissionScheduleState::Started
            | MissionScheduleState::Refused
            | MissionScheduleState::Attention => {
                return Err("only a pending Mission schedule can be cancelled");
            }
        }
        Ok(Self::snapshot_response(
            &snapshot,
            self.active.get(&mission_id),
        ))
    }

    /// Earliest wall-clock instant that can change a schedule without a new request.
    pub(crate) fn next_schedule_wake(ledger: &Ledger) -> Result<Option<u64>, String> {
        let listed = ledger
            .list(runtrol_ledger::MAX_QUERY_MISSIONS)
            .map_err(|error| error.to_string())?;
        if listed.truncated {
            return Err("the Mission schedule query exceeded its fixed bound".to_owned());
        }
        let mut next = None;
        for snapshot in listed.missions {
            let Some(schedule) = snapshot.mission.schedule else {
                continue;
            };
            let candidate = match schedule.state {
                MissionScheduleState::Pending => Some(schedule.due_unix_ms),
                MissionScheduleState::Launching
                    if snapshot.mission.state == MissionState::Blocked
                        || snapshot
                            .tasks
                            .iter()
                            .any(|task| task.state == TaskState::Reserved) =>
                {
                    Some(0)
                }
                MissionScheduleState::Launching
                | MissionScheduleState::Started
                | MissionScheduleState::Cancelled
                | MissionScheduleState::Refused
                | MissionScheduleState::Attention => None,
            };
            if let Some(candidate) = candidate {
                next = Some(next.map_or(candidate, |current: u64| current.min(candidate)));
            }
        }
        Ok(next)
    }

    /// Atomically claim one due schedule and start its existing Mission scheduler.
    pub(crate) fn claim_due_schedule(
        &mut self,
        ledger: &Ledger,
        growth: &mut crate::growth::GrowthController,
        now_unix_ms: u64,
    ) -> Result<Option<MissionScheduleExecution>, String> {
        let listed = ledger
            .list(runtrol_ledger::MAX_QUERY_MISSIONS)
            .map_err(|error| error.to_string())?;
        if listed.truncated {
            return Err("the Mission schedule query exceeded its fixed bound".to_owned());
        }

        for mut snapshot in listed.missions.iter().cloned() {
            let Some(schedule) = snapshot.mission.schedule.as_mut() else {
                continue;
            };
            if schedule.state != MissionScheduleState::Launching {
                continue;
            }
            if snapshot.mission.state == MissionState::Blocked {
                schedule.state = MissionScheduleState::Attention;
                schedule.failure = Some("interruptedLaunchRequiresRecovery".into());
                ledger.put(&snapshot).map_err(|error| error.to_string())?;
                continue;
            }
            if snapshot.mission.state == MissionState::Running
                && snapshot
                    .tasks
                    .iter()
                    .any(|task| task.state == TaskState::Reserved)
            {
                return Ok(Some(schedule_execution(&snapshot)?));
            }
        }

        let due = listed
            .missions
            .into_iter()
            .filter(|snapshot| {
                snapshot.mission.schedule.as_ref().is_some_and(|schedule| {
                    schedule.state == MissionScheduleState::Pending
                        && schedule.due_unix_ms <= now_unix_ms
                })
            })
            .min_by_key(|snapshot| {
                snapshot
                    .mission
                    .schedule
                    .as_ref()
                    .map_or((u64::MAX, String::new()), |schedule| {
                        (schedule.due_unix_ms, schedule.id.to_string())
                    })
            });
        let Some(mut due) = due else {
            return Ok(None);
        };
        let mission_id = due.mission.id;
        let schedule = due
            .mission
            .schedule
            .as_ref()
            .ok_or_else(|| "the due schedule disappeared".to_owned())?
            .clone();
        let Ok(approved) = growth.approved_capabilities(&due.mission.project_id) else {
            refuse_due_schedule(ledger, &mut due, "capabilityAuthorityChanged")?;
            return Ok(None);
        };
        let started = self.start_snapshot(
            ledger,
            &approved,
            mission_id,
            &hex(&schedule.mission_sha256),
            false,
        );
        let Ok(mut started) = started else {
            refuse_due_schedule(ledger, &mut due, "reviewAuthorityChanged")?;
            return Ok(None);
        };
        let current = started
            .mission
            .schedule
            .as_mut()
            .ok_or_else(|| "the claimed schedule disappeared".to_owned())?;
        if !same_schedule_authority(current, &schedule)
            || current.state != MissionScheduleState::Pending
            || schedule.policy_sha256 != started.mission.policy_sha256
        {
            return Err("the due schedule authority changed while it was claimed".to_owned());
        }
        current.state = MissionScheduleState::Launching;
        current.claimed_unix_ms = Some(now_unix_ms);
        current.failure = None;
        ledger.put(&started).map_err(|error| error.to_string())?;
        Ok(Some(schedule_execution(&started)?))
    }

    /// Close one claimed first-wave launch with only a structural result code.
    pub(crate) fn finish_schedule_launch(
        ledger: &Ledger,
        mission_id: &str,
        schedule_id: &str,
        failure: Option<&'static str>,
    ) -> Result<(), String> {
        let mission_id: MissionId = mission_id
            .parse()
            .map_err(|_| "the Mission identity is invalid".to_owned())?;
        let schedule_id: ScheduleId = schedule_id
            .parse()
            .map_err(|_| "the schedule identity is invalid".to_owned())?;
        let mut snapshot = ledger
            .snapshot(mission_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the Mission does not exist".to_owned())?;
        let schedule = snapshot
            .mission
            .schedule
            .as_mut()
            .ok_or_else(|| "the Mission schedule does not exist".to_owned())?;
        if schedule.id != schedule_id || schedule.state != MissionScheduleState::Launching {
            return Err("the Mission schedule launch authority changed".to_owned());
        }
        if let Some(failure) = failure {
            schedule.state = MissionScheduleState::Attention;
            schedule.failure = Some(failure.into());
        } else {
            schedule.state = MissionScheduleState::Started;
            schedule.failure = None;
        }
        ledger.put(&snapshot).map_err(|error| error.to_string())
    }
}

fn reviewed_schedule_providers(
    active: &ActiveMission,
    runtime_ids: &[Box<str>],
    providers: &[MissionScheduleProviderLine],
) -> Result<BTreeMap<TaskId, Box<str>>, &'static str> {
    if providers.len() != active.validated.tasks.len() {
        return Err("the schedule must assign every reviewed Task exactly once");
    }
    let mut assignments = BTreeMap::new();
    for provider in providers {
        let task_id: TaskId = provider
            .task_id
            .parse()
            .map_err(|_| "a scheduled Task identity is invalid")?;
        let task = active
            .validated
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or("a scheduled Task is not in the reviewed Mission")?;
        if !runtime_ids
            .iter()
            .any(|runtime| runtime.as_ref() == provider.provider_runtime_id.as_ref())
        {
            return Err("a scheduled provider is not currently runtime-discovered");
        }
        if let ProviderSelector::Exact(expected) = &task.provider_selector
            && expected.as_ref() != provider.provider_runtime_id.as_ref()
        {
            return Err("a scheduled provider differs from the reviewed exact selector");
        }
        if assignments
            .insert(task_id, provider.provider_runtime_id.clone())
            .is_some()
        {
            return Err("a scheduled Task was assigned more than once");
        }
    }
    if active
        .validated
        .tasks
        .iter()
        .any(|task| !assignments.contains_key(&task.id))
    {
        return Err("the schedule omitted a reviewed Task");
    }
    Ok(assignments)
}

fn same_schedule_authority(left: &MissionSchedule, right: &MissionSchedule) -> bool {
    left.id == right.id
        && left.due_unix_ms == right.due_unix_ms
        && left.mission_sha256 == right.mission_sha256
        && left.policy_sha256 == right.policy_sha256
        && left.providers == right.providers
}

fn schedule_execution(snapshot: &LedgerSnapshot) -> Result<MissionScheduleExecution, String> {
    let schedule = snapshot
        .mission
        .schedule
        .as_ref()
        .ok_or_else(|| "the Mission schedule does not exist".to_owned())?;
    let providers = schedule
        .providers
        .iter()
        .map(|provider| {
            (
                provider.task_id.to_string().into(),
                provider.provider_runtime_id.clone(),
            )
        })
        .collect();
    Ok(MissionScheduleExecution {
        mission_id: snapshot.mission.id.to_string().into(),
        schedule_id: schedule.id.to_string().into(),
        providers,
    })
}

fn refuse_due_schedule(
    ledger: &Ledger,
    snapshot: &mut LedgerSnapshot,
    failure: &'static str,
) -> Result<(), String> {
    let schedule = snapshot
        .mission
        .schedule
        .as_mut()
        .ok_or_else(|| "the due schedule disappeared".to_owned())?;
    if schedule.state != MissionScheduleState::Pending {
        return Err("the due schedule is no longer pending".to_owned());
    }
    schedule.state = MissionScheduleState::Refused;
    schedule.failure = Some(failure.into());
    ledger.put(snapshot).map_err(|error| error.to_string())
}

pub(super) fn schedule_line(schedule: &MissionSchedule) -> MissionScheduleLine {
    MissionScheduleLine {
        schedule_id: schedule.id.to_string().into(),
        due_unix_ms: schedule.due_unix_ms,
        state: schedule_state(schedule.state).into(),
        providers: schedule
            .providers
            .iter()
            .map(|provider| MissionScheduleProviderLine {
                task_id: provider.task_id.to_string().into(),
                provider_runtime_id: provider.provider_runtime_id.clone(),
            })
            .collect(),
        failure: schedule.failure.clone(),
    }
}

const fn schedule_state(state: MissionScheduleState) -> &'static str {
    match state {
        MissionScheduleState::Pending => "pending",
        MissionScheduleState::Launching => "launching",
        MissionScheduleState::Started => "started",
        MissionScheduleState::Cancelled => "cancelled",
        MissionScheduleState::Refused => "refused",
        MissionScheduleState::Attention => "attention",
    }
}
