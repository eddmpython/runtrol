import type {
  MissionScheduleProviderLine,
  MissionSnapshot,
} from "../protocol";

export const MIN_SCHEDULE_LEAD_MS = 1_000;
export const MAX_SCHEDULE_LEAD_MS = 366 * 24 * 60 * 60 * 1_000;

export type MissionScheduleReview = {
  readonly scheduleId: string;
  readonly replacesScheduleId: string | null;
  readonly missionId: string;
  readonly missionSha256: string;
  readonly dueUnixMs: number;
  readonly providers: readonly MissionScheduleProviderLine[];
  readonly authority: string;
};

export function reviewMissionSchedule(
  snapshot: MissionSnapshot,
  scheduleId: string,
  dueUnixMs: number,
  assignments: ReadonlyMap<string, string>,
  nowUnixMs: number,
): MissionScheduleReview {
  if (!scheduleId.startsWith("sch_") || scheduleId.length > 64) {
    throw new Error("invalid schedule identity");
  }
  if (snapshot.mission.state !== "validated") {
    throw new Error("Mission must be validated before scheduling");
  }
  if (!Number.isSafeInteger(dueUnixMs)
    || dueUnixMs < nowUnixMs + MIN_SCHEDULE_LEAD_MS
    || dueUnixMs > nowUnixMs + MAX_SCHEDULE_LEAD_MS) {
    throw new Error("the schedule must be between one second and 366 days from now");
  }
  const providers = snapshot.tasks.map((task) => {
    const provider = assignments.get(task.task_id);
    if (!provider) throw new Error(`Task ${task.key} has no reviewed provider assignment`);
    return { task_id: task.task_id, provider_runtime_id: provider };
  });
  if (assignments.size !== providers.length) {
    throw new Error("the schedule contains a Task outside the reviewed Mission");
  }
  const replacesScheduleId = snapshot.mission.schedule?.state === "pending"
    ? snapshot.mission.schedule.schedule_id
    : null;
  const authority = scheduleAuthority(snapshot, replacesScheduleId, dueUnixMs, providers);
  return {
    scheduleId,
    replacesScheduleId,
    missionId: snapshot.mission.mission_id,
    missionSha256: snapshot.mission_sha256,
    dueUnixMs,
    providers,
    authority,
  };
}

export function assertMissionScheduleAuthority(
  review: MissionScheduleReview,
  current: MissionSnapshot,
): void {
  const currentPending = current.mission.schedule?.state === "pending"
    ? current.mission.schedule.schedule_id
    : null;
  if (review.missionId !== current.mission.mission_id
    || review.authority !== scheduleAuthority(current, currentPending, review.dueUnixMs, review.providers)) {
    throw new Error("the Mission schedule authority changed after review");
  }
}

export function parseLocalScheduleInput(text: string): number | null {
  const normalized = text.trim().replace("T", " ");
  const match = /^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2})$/u.exec(normalized);
  if (!match) return null;
  const [year, month, day, hour, minute] = match.slice(1).map(Number);
  const local = new Date(year, month - 1, day, hour, minute, 0, 0);
  return localScheduleInput(local.getTime()) === normalized ? local.getTime() : null;
}

export function localScheduleInput(unixMs: number): string {
  const date = new Date(unixMs);
  const part = (value: number) => value.toString().padStart(2, "0");
  return `${date.getFullYear()}-${part(date.getMonth() + 1)}-${part(date.getDate())} ${part(date.getHours())}:${part(date.getMinutes())}`;
}

export function tomorrowAtNine(nowUnixMs: number): number {
  const date = new Date(nowUnixMs);
  date.setDate(date.getDate() + 1);
  return date.setHours(9, 0, 0, 0);
}

function scheduleAuthority(
  snapshot: MissionSnapshot,
  replacesScheduleId: string | null,
  dueUnixMs: number,
  providers: readonly MissionScheduleProviderLine[],
): string {
  return JSON.stringify([
    snapshot.mission.mission_id,
    snapshot.mission_sha256,
    snapshot.policy_sha256,
    snapshot.mission.state,
    replacesScheduleId,
    dueUnixMs,
    snapshot.tasks.map((task) => [
      task.task_id,
      task.instruction_sha256,
      task.provider_selector,
      task.workspace_mode,
    ]),
    providers,
  ]);
}
