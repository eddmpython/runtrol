import type { MissionSnapshot, MissionTaskLine } from "../protocol";

export type MissionWaveProgress = {
  report(value: { readonly message?: string; readonly increment?: number }): void;
};

export type MissionWaveResult = {
  readonly snapshot: MissionSnapshot;
  readonly sessionIds: readonly string[];
};

export type MissionWavePort = {
  prepare(
    missionId: string,
    task: MissionTaskLine,
    provider: string,
  ): Promise<{ readonly snapshot: MissionSnapshot; readonly sessionId: string }>;
  hasAmbiguousSubmission(taskId: string): boolean;
  markAmbiguousSubmission(taskId: string): Promise<void>;
  resolveInstruction(
    missionId: string,
    task: MissionTaskLine,
  ): Promise<{ readonly sessionId: string; readonly instruction: string }>;
  submit(sessionId: string, instruction: string): Promise<void>;
  clearAmbiguousSubmission(taskId: string): Promise<void>;
  getSnapshot(missionId: string): Promise<MissionSnapshot>;
};

/// Prepare and send one exact scheduler wave without knowing which provider implementation owns its sessions.
export class MissionWaveRunner {
  constructor(private readonly port: MissionWavePort) {}

  async run(
    initial: MissionSnapshot,
    plannedTasks: readonly MissionTaskLine[],
    assignments: ReadonlyMap<string, string>,
    progress: MissionWaveProgress,
    requireEveryTask: boolean,
  ): Promise<MissionWaveResult> {
    let current = initial;
    const taskIds = new Set(plannedTasks.map((task) => task.task_id));
    const sessionIds: string[] = [];
    for (const planned of plannedTasks) {
      const exact = current.tasks.find((task) => task.task_id === planned.task_id);
      if (!exact) throw new Error(`reviewed Task ${planned.key} is no longer present`);
      if (exact.state === "reserved") {
        progress.report({
          message: `Preparing ${exact.key}`,
          increment: plannedTasks.length === 0 ? 0 : 45 / plannedTasks.length,
        });
        const prepared = await this.port.prepare(
          current.mission.mission_id,
          exact,
          providerAssignment(assignments, exact),
        );
        current = prepared.snapshot;
        sessionIds.push(prepared.sessionId);
        continue;
      }
      if (exact.state === "awaitingInput" && exact.session_id) {
        sessionIds.push(exact.session_id);
        continue;
      }
      if (exact.state === "running") continue;
      if (requireEveryTask) {
        throw new Error(`reviewed Task ${exact.key} is ${exact.state}, not reserved for this wave`);
      }
    }

    const ready = current.tasks.filter((task) => taskIds.has(task.task_id) && task.state === "awaitingInput");
    const instructions = [];
    for (const task of ready) {
      if (this.port.hasAmbiguousSubmission(task.task_id)) {
        throw new Error(`Task ${task.key} has an ambiguous prior Send and requires explicit Task recovery`);
      }
      progress.report({
        message: `Rechecking ${task.instruction_ref}`,
        increment: ready.length === 0 ? 0 : 20 / ready.length,
      });
      await this.port.markAmbiguousSubmission(task.task_id);
      instructions.push({ task, instruction: await this.port.resolveInstruction(current.mission.mission_id, task) });
    }
    progress.report({ message: "Sending exact reviewed instructions", increment: 20 });
    const submissions = await Promise.allSettled(instructions.map(async ({ task, instruction }) => {
      await this.port.submit(instruction.sessionId, instruction.instruction);
      await this.port.clearAmbiguousSubmission(task.task_id);
      return instruction.sessionId;
    }));
    const sent = submissions.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
    const failed = submissions.length - sent.length;
    if (failed > 0) {
      throw new Error(
        `${failed} of ${submissions.length} provider submissions are ambiguous; Mission state was kept for explicit recovery`,
      );
    }
    current = await this.port.getSnapshot(current.mission.mission_id);
    return { snapshot: current, sessionIds: [...new Set([...sessionIds, ...sent])] };
  }
}

function providerAssignment(assignments: ReadonlyMap<string, string>, task: MissionTaskLine): string {
  const provider = assignments.get(task.task_id);
  if (!provider) throw new Error(`Task ${task.key} has no reviewed provider assignment`);
  return provider;
}
