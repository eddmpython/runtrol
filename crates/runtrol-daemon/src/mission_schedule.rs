//! Core-owned wall-clock wake and first-wave execution through the existing local contracts.
//!
//! This module owns no provider implementation and no conversation state. At the due instant it claims exact
//! metadata in the Mission ledger, then behaves like a local surface over the same IPC used by the CLI and Studio.

use std::{sync::Arc, time::Duration};

use runtrol_ipc::wire::{MissionSnapshot, MissionTaskLine, Request, Response};
use runtrol_provider::WorkspaceAccess;
use tokio::sync::Notify;

use crate::{
    Composed,
    mission::{MissionController, MissionScheduleExecution},
};

const CLOCK_RECHECK: Duration = Duration::from_mins(1);
const LOCK_RETRY: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchFailure {
    CoreConnection,
    MissionAuthority,
    WorkspacePreparation,
    ProviderStart,
    SessionBinding,
    InstructionAuthority,
    SubmissionAmbiguous,
}

impl LaunchFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::CoreConnection => "coreConnectionLost",
            Self::MissionAuthority => "missionAuthorityChanged",
            Self::WorkspacePreparation => "workspacePreparationFailed",
            Self::ProviderStart => "providerStartFailed",
            Self::SessionBinding => "sessionBindingFailed",
            Self::InstructionAuthority => "instructionAuthorityChanged",
            Self::SubmissionAmbiguous => "providerSubmissionAmbiguous",
        }
    }
}

/// Run the single Core-owned Mission schedule actor until daemon shutdown aborts it.
pub(crate) async fn supervise(composed: Arc<Composed>, address: String, wake: Arc<Notify>) {
    loop {
        let next = MissionController::next_schedule_wake(&composed.ledger);
        let Ok(next) = next else {
            tokio::select! {
                () = wake.notified() => {}
                () = tokio::time::sleep(CLOCK_RECHECK) => {}
            }
            continue;
        };
        let Some(due_unix_ms) = next else {
            wake.notified().await;
            continue;
        };
        if let Some(wait) = wait_until(due_unix_ms, unix_ms()) {
            tokio::select! {
                () = wake.notified() => {}
                () = tokio::time::sleep(wait.min(CLOCK_RECHECK)) => {}
            }
            continue;
        }

        let execution = {
            let Ok(mut growth) = composed.growth.try_lock() else {
                tokio::time::sleep(LOCK_RETRY).await;
                continue;
            };
            let Ok(mut controller) = composed.missions.try_lock() else {
                drop(growth);
                tokio::time::sleep(LOCK_RETRY).await;
                continue;
            };
            controller.claim_due_schedule(&composed.ledger, &mut growth, unix_ms())
        };
        let execution = match execution {
            Ok(Some(execution)) => execution,
            Ok(None) => continue,
            Err(_) => {
                tokio::time::sleep(CLOCK_RECHECK).await;
                continue;
            }
        };
        let failure = run_first_wave(&address, &execution).await.err();
        loop {
            let finished = match composed.missions.try_lock() {
                Ok(_controller) => Some(MissionController::finish_schedule_launch(
                    &composed.ledger,
                    &execution.mission_id,
                    &execution.schedule_id,
                    failure.map(LaunchFailure::code),
                )),
                Err(_) => None,
            };
            match finished {
                Some(Ok(())) => break,
                Some(Err(_)) | None => tokio::time::sleep(LOCK_RETRY).await,
            }
        }
    }
}

fn wait_until(due_unix_ms: u64, now_unix_ms: u64) -> Option<Duration> {
    due_unix_ms
        .checked_sub(now_unix_ms)
        .filter(|remaining| *remaining > 0)
        .map(Duration::from_millis)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn run_first_wave(
    address: &str,
    execution: &MissionScheduleExecution,
) -> Result<(), LaunchFailure> {
    let mut connection = runtrol_ipc::connect(address)
        .await
        .map_err(|_| LaunchFailure::CoreConnection)?;
    match exchange(
        &mut connection,
        &Request::Hello {
            wire: runtrol_ipc::WIRE_VERSION,
        },
    )
    .await?
    {
        Response::Welcome { .. } => {}
        _ => return Err(LaunchFailure::CoreConnection),
    }
    let snapshot = mission_snapshot(
        exchange(
            &mut connection,
            &Request::MissionGet {
                mission_id: execution.mission_id.clone(),
            },
        )
        .await?,
    )?;
    if snapshot.mission.state.as_ref() != "running"
        || snapshot.mission.schedule.as_ref().is_none_or(|schedule| {
            schedule.schedule_id.as_ref() != execution.schedule_id.as_ref()
                || schedule.state.as_ref() != "launching"
        })
    {
        return Err(LaunchFailure::MissionAuthority);
    }
    let reserved: Vec<_> = snapshot
        .tasks
        .into_iter()
        .filter(|task| task.state.as_ref() == "reserved")
        .collect();
    if reserved.is_empty() {
        return Err(LaunchFailure::MissionAuthority);
    }
    for task in reserved {
        run_reserved_task(&mut connection, execution, task).await?;
    }
    Ok(())
}

async fn run_reserved_task(
    connection: &mut runtrol_ipc::Connection,
    execution: &MissionScheduleExecution,
    task: MissionTaskLine,
) -> Result<(), LaunchFailure> {
    let provider = execution
        .providers
        .get(&task.task_id)
        .ok_or(LaunchFailure::MissionAuthority)?;
    let Response::MissionWorkspace(workspace) = exchange(
        connection,
        &Request::MissionPrepareTask {
            mission_id: execution.mission_id.clone(),
            task_id: task.task_id.clone(),
        },
    )
    .await?
    else {
        return Err(LaunchFailure::WorkspacePreparation);
    };
    let Response::Started { session } = exchange(
        connection,
        &Request::Start {
            provider: provider.clone(),
            workspace: workspace.workspace.clone(),
            workspace_access: if task.workspace_mode.as_ref() == "readOnlyBase" {
                WorkspaceAccess::Shared
            } else {
                WorkspaceAccess::Exclusive
            },
            model: None,
            permission: None,
        },
    )
    .await?
    else {
        return Err(LaunchFailure::ProviderStart);
    };
    let Response::Mission(_) = exchange(
        connection,
        &Request::MissionBindSession {
            mission_id: execution.mission_id.clone(),
            task_id: task.task_id.clone(),
            session_id: session.to_string().into(),
            provider_runtime_id: provider.clone(),
            native_session_id: None,
            workspace: workspace.workspace.clone(),
        },
    )
    .await?
    else {
        return Err(LaunchFailure::SessionBinding);
    };
    let Response::MissionInstruction(instruction) = exchange(
        connection,
        &Request::MissionSendTaskInstruction {
            mission_id: execution.mission_id.clone(),
            task_id: task.task_id,
            instruction_sha256: task.instruction_sha256,
        },
    )
    .await?
    else {
        return Err(LaunchFailure::InstructionAuthority);
    };
    if instruction.session_id.as_ref() != session.to_string() {
        return Err(LaunchFailure::InstructionAuthority);
    }
    match exchange(
        connection,
        &Request::Prompt {
            session,
            text: instruction.instruction,
        },
    )
    .await
    {
        Ok(Response::Done) => Ok(()),
        Ok(_) | Err(_) => Err(LaunchFailure::SubmissionAmbiguous),
    }
}

async fn exchange(
    connection: &mut runtrol_ipc::Connection,
    request: &Request,
) -> Result<Response, LaunchFailure> {
    let frame = serde_json::to_vec(request).map_err(|_| LaunchFailure::CoreConnection)?;
    connection
        .send(&frame)
        .await
        .map_err(|_| LaunchFailure::CoreConnection)?;
    let answer = connection
        .recv()
        .await
        .map_err(|_| LaunchFailure::CoreConnection)?
        .ok_or(LaunchFailure::CoreConnection)?;
    let response = serde_json::from_slice(&answer).map_err(|_| LaunchFailure::CoreConnection)?;
    Ok(response)
}

fn mission_snapshot(response: Response) -> Result<Box<MissionSnapshot>, LaunchFailure> {
    match response {
        Response::Mission(snapshot) => Ok(snapshot),
        _ => Err(LaunchFailure::MissionAuthority),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_clock_waits_only_for_future_instants() {
        assert_eq!(wait_until(12_000, 10_000), Some(Duration::from_secs(2)));
        assert_eq!(wait_until(10_000, 10_000), None);
        assert_eq!(wait_until(9_000, 10_000), None);
    }

    #[test]
    fn failures_expose_only_closed_structural_codes() {
        let codes = [
            LaunchFailure::CoreConnection,
            LaunchFailure::MissionAuthority,
            LaunchFailure::WorkspacePreparation,
            LaunchFailure::ProviderStart,
            LaunchFailure::SessionBinding,
            LaunchFailure::InstructionAuthority,
            LaunchFailure::SubmissionAmbiguous,
        ]
        .map(LaunchFailure::code);
        assert_eq!(codes.len(), 7);
        assert!(codes.iter().all(|code| !code.contains(' ')));
    }
}
