import type { MissionSnapshot, MissionTaskLine } from "../protocol";

const RECOVERY_TASK_STATES = new Set(["blocked", "eligible", "reserved"]);

export type InterruptedRecoveryTask = {
  readonly taskId: string;
  readonly key: string;
  readonly state: string;
  readonly instructionSha256: string;
  readonly providerSelector: string;
  readonly workspaceMode: MissionTaskLine["workspace_mode"];
  readonly workspace: string;
  readonly baseCommit: string;
};

export type InterruptedRecoveryPlan = {
  readonly missionId: string;
  readonly missionName: string;
  readonly project: string;
  readonly missionSha256: string;
  readonly policySha256: string;
  readonly completionPolicy: "allTasks" | "chooseOne";
  readonly tasks: readonly InterruptedRecoveryTask[];
};

/// Freeze the exact local authority for recovering one Core-interrupted Mission.
///
/// The eligible and reserved states are included because a second Core interruption can happen between reopening
/// blocked Tasks and resuming their scheduler. They contain no provider input and are safe to finish under the same
/// explicit recovery warning. Instructions and conversation data are deliberately absent.
export function interruptedRecoveryPlan(snapshot: MissionSnapshot): InterruptedRecoveryPlan {
  if (snapshot.mission.state !== "blocked") {
    throw new Error("only a recovery-blocked Mission can use interrupted recovery");
  }
  if (snapshot.mission.completion_policy === "unavailableAfterRestart") {
    throw new Error("the reviewed Mission contract changed after restart; validate it again or cancel the Mission");
  }
  const tasks = snapshot.tasks.filter((task) => RECOVERY_TASK_STATES.has(task.state));
  if (tasks.length === 0) {
    throw new Error("the Mission has no interrupted Task to recover");
  }
  return {
    missionId: snapshot.mission.mission_id,
    missionName: snapshot.mission.name,
    project: snapshot.mission.project,
    missionSha256: snapshot.mission_sha256,
    policySha256: snapshot.policy_sha256,
    completionPolicy: snapshot.mission.completion_policy,
    tasks: tasks.map(exactTask),
  };
}

/// Require that the bytes and every launch-relevant Task fact still match the operator's confirmation.
export function assertInterruptedRecoveryAuthority(
  plan: InterruptedRecoveryPlan,
  snapshot: MissionSnapshot,
): void {
  const current = interruptedRecoveryPlan(snapshot);
  if (JSON.stringify(current) !== JSON.stringify(plan)) {
    throw new Error("the interrupted Mission changed after review; recovery was not started");
  }
}

export function recoveryTasks(
  plan: InterruptedRecoveryPlan,
  snapshot: MissionSnapshot,
): MissionTaskLine[] {
  const byId = new Map(snapshot.tasks.map((task) => [task.task_id, task]));
  return plan.tasks.map((planned) => {
    const task = byId.get(planned.taskId);
    if (!task) throw new Error(`recovery Task ${planned.key} is no longer present`);
    return task;
  });
}

export function interruptedRecoveryDetail(
  plan: InterruptedRecoveryPlan,
  assignments: ReadonlyMap<string, string>,
): string {
  const tasks = plan.tasks.map((task) => {
    const provider = assignments.get(task.taskId);
    if (!provider) throw new Error(`recovery Task ${task.key} has no reviewed provider assignment`);
    return `${task.key}: ${provider}\n  ${task.workspace}`;
  }).join("\n");
  return [
    `Mission SHA-256 ${plan.missionSha256}`,
    `Policy SHA-256 ${plan.policySha256}`,
    `Project ${plan.project}`,
    "",
    tasks,
    "",
    "The previous provider input may already have caused external effects.",
    "Recovery starts fresh sessions and may repeat those effects. Exact instructions are rechecked before Send.",
  ].join("\n");
}

function exactTask(task: MissionTaskLine): InterruptedRecoveryTask {
  if (task.workspace_mode === "unavailableAfterRestart" || !task.workspace || !task.base_commit) {
    throw new Error(`Task ${task.key} lost its reviewed workspace authority; validate again or cancel`);
  }
  return {
    taskId: task.task_id,
    key: task.key,
    state: task.state,
    instructionSha256: task.instruction_sha256,
    providerSelector: task.provider_selector,
    workspaceMode: task.workspace_mode,
    workspace: task.workspace,
    baseCommit: task.base_commit,
  };
}
