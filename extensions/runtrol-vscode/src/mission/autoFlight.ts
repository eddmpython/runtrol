import type { SessionDescriptor } from "@runtrol/runtime-client";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import { hasMissionMomentumWork, type MissionMomentum } from "./momentum";

export const MAX_AUTO_FLIGHTS = 8;
const MAX_AUTO_FLIGHT_TASKS = 64;
const MAX_ID_LENGTH = 160;

type AutoFlightSession = Pick<
  SessionDescriptor,
  "sessionId" | "sessionGeneration" | "lifecycle" | "waitingOn"
>;

export type AutoFlightTurn = {
  readonly taskId: string;
  readonly sessionId: string;
  readonly sessionGeneration: number;
};

export type AutoFlightArm = {
  readonly missionId: string;
  readonly missionSha256: string;
  readonly operatorChoiceProvider: string | null;
  readonly idleAuthorizedTaskIds: readonly string[];
  readonly turns: readonly AutoFlightTurn[];
};

export type AutoFlightDecision =
  | { readonly kind: "advance"; readonly momentum: MissionMomentum }
  | { readonly kind: "wait" }
  | { readonly kind: "arrived" }
  | { readonly kind: "disarm"; readonly reason: string };

export type AutoFlightWriter = (arms: readonly AutoFlightArm[]) => PromiseLike<void>;

/// Durable bounded Auto Flight authority. It stores identities and lifecycle generations, never instructions or
/// conversation content. Writes serialize so a turn observation and an operator disarm cannot resurrect each other.
export class AutoFlights {
  private readonly arms = new Map<string, AutoFlightArm>();
  private persistence: Promise<void> = Promise.resolve();

  constructor(initial: unknown, private readonly write: AutoFlightWriter) {
    for (const arm of readAutoFlightArms(initial)) this.arms.set(arm.missionId, arm);
  }

  current(): readonly AutoFlightArm[] {
    return [...this.arms.values()].sort(compareArms);
  }

  get(missionId: string): AutoFlightArm | null {
    return this.arms.get(missionId) ?? null;
  }

  isArmed(missionId: string): boolean {
    return this.arms.has(missionId);
  }

  arm(value: AutoFlightArm): Promise<void> {
    return this.armMany([value]);
  }

  armMany(values: readonly AutoFlightArm[]): Promise<void> {
    return this.persist((next) => {
      for (const value of values) next.set(value.missionId, value);
      if (next.size > MAX_AUTO_FLIGHTS) {
        throw new Error(`at most ${MAX_AUTO_FLIGHTS} Mission Auto Flights can be armed`);
      }
    });
  }

  disarm(missionId: string): Promise<void> {
    return this.persist((next) => {
      next.delete(missionId);
    });
  }

  recordSubmissions(missionId: string, submissions: readonly AutoFlightTurn[]): Promise<void> {
    return this.persist((next) => {
      const arm = next.get(missionId);
      if (!arm) throw new Error("the Mission Auto Flight is no longer armed");
      next.set(missionId, recordAutoFlightSubmissions(arm, submissions));
    });
  }

  reconcile(missionId: string, snapshot: MissionSnapshot): Promise<void> {
    return this.persist((next) => {
      const arm = next.get(missionId);
      if (arm) next.set(missionId, reconcileAutoFlightArm(arm, snapshot));
    });
  }

  private persist(change: (next: Map<string, AutoFlightArm>) => void): Promise<void> {
    const update = this.persistence.then(async () => {
      const next = new Map(this.arms);
      change(next);
      const values = [...next.values()].sort(compareArms);
      if (sameArms(values, this.current())) return;
      await this.write(values);
      this.arms.clear();
      for (const arm of values) this.arms.set(arm.missionId, arm);
    });
    this.persistence = update.then(
      () => undefined,
      () => undefined,
    );
    return update;
  }
}

/// Mint one exact arm from the state the operator reviewed. Existing idle Tasks receive one explicit authorization;
/// running Tasks instead capture the generation that must advance before an automatic Gate may run.
export function createAutoFlightArm(
  snapshot: MissionSnapshot,
  operatorChoiceProvider: string | null,
  sessions: readonly AutoFlightSession[],
  momentum: MissionMomentum,
  nowUnixMs: number,
): AutoFlightArm {
  if (snapshot.mission.completion_policy !== "allTasks") {
    throw new Error("a choose-one Mission keeps its explicit Fleet Compare flow");
  }
  if (snapshot.mission.state !== "validated" && snapshot.mission.state !== "running") {
    throw new Error("only a validated or running ordinary Mission can arm Auto Flight");
  }
  if (snapshot.mission.state === "validated" && snapshot.approval_expires_unix_ms <= nowUnixMs) {
    throw new Error("the Mission review expired; validate it again before arming Auto Flight");
  }
  if (momentum.stopped || momentum.manual.length > 0 || (!hasMissionMomentumWork(momentum) && momentum.waiting.length === 0)) {
    throw new Error("the Mission has no safe reviewed Auto Flight path");
  }

  const sessionsById = new Map(sessions.map((session) => [session.sessionId, session]));
  const idleAuthorizedTaskIds: string[] = [];
  const turns: AutoFlightTurn[] = [];
  for (const task of snapshot.tasks) {
    if (task.state !== "running") continue;
    const session = task.session_id ? sessionsById.get(task.session_id) : null;
    if (!session || (session.lifecycle !== "hotIdle" && session.lifecycle !== "hotRunning")) {
      throw new Error(`Task ${task.key} has no exact live Runtime turn for Auto Flight`);
    }
    if (session.lifecycle === "hotIdle" && !session.waitingOn) {
      idleAuthorizedTaskIds.push(task.task_id);
    } else {
      turns.push(turnMarker(task, session));
    }
  }
  return {
    missionId: snapshot.mission.mission_id,
    missionSha256: snapshot.mission_sha256,
    operatorChoiceProvider,
    idleAuthorizedTaskIds: idleAuthorizedTaskIds.sort(),
    turns: turns.sort(compareTurns),
  };
}

/// Decide whether one arm may advance now. An idle session sent by Auto Flight is not completion evidence until its
/// exact lifecycle generation has advanced. Person and quota waits remain armed and naturally resume on a later row.
export function decideAutoFlight(
  arm: AutoFlightArm,
  snapshot: MissionSnapshot,
  momentum: MissionMomentum,
  sessions: readonly AutoFlightSession[],
  nowUnixMs: number,
): AutoFlightDecision {
  if (snapshot.mission.mission_id !== arm.missionId || snapshot.mission_sha256 !== arm.missionSha256) {
    return { kind: "disarm", reason: "the reviewed Mission identity changed" };
  }
  if (snapshot.mission.completion_policy !== "allTasks") {
    return { kind: "disarm", reason: "the Mission now requires a specialized flow" };
  }
  if (snapshot.mission.state === "integrating") return { kind: "arrived" };
  if (snapshot.mission.state === "paused") return { kind: "wait" };
  if (snapshot.mission.state === "validated" && snapshot.approval_expires_unix_ms <= nowUnixMs) {
    return { kind: "disarm", reason: "the local Mission review expired" };
  }
  if (momentum.stopped) return { kind: "disarm", reason: `the Mission is ${momentum.stopped}` };
  if (momentum.manual.length > 0) {
    return { kind: "disarm", reason: `${momentum.manual.length} Tasks require explicit recovery` };
  }

  const sessionsById = new Map(sessions.map((session) => [session.sessionId, session]));
  const idle = new Set(arm.idleAuthorizedTaskIds);
  const turns = new Map(arm.turns.map((turn) => [turn.taskId, turn]));
  const authorized = new Set<string>();
  for (const task of momentum.verify) {
    if (idle.has(task.task_id)) {
      authorized.add(task.task_id);
      continue;
    }
    const expected = turns.get(task.task_id);
    const session = expected ? sessionsById.get(expected.sessionId) : null;
    if (!expected || !session || task.session_id !== expected.sessionId) {
      return { kind: "disarm", reason: `Task ${task.key} has no proven Auto Flight turn` };
    }
    if (session.lifecycle === "hotIdle" && session.sessionGeneration > expected.sessionGeneration) {
      authorized.add(task.task_id);
    }
  }
  const restricted = restrictAutoFlightMomentum(momentum, authorized);
  return hasMissionMomentumWork(restricted)
    ? { kind: "advance", momentum: restricted }
    : { kind: "wait" };
}

export function restrictAutoFlightMomentum(
  momentum: MissionMomentum,
  authorizedTaskIds: ReadonlySet<string>,
): MissionMomentum {
  const verify = momentum.verify.filter((task) => authorizedTaskIds.has(task.task_id));
  return {
    ...momentum,
    verify,
    waiting: [...momentum.waiting, ...momentum.verify.filter((task) => !authorizedTaskIds.has(task.task_id))],
  };
}

export function recordAutoFlightSubmissions(
  arm: AutoFlightArm,
  submissions: readonly AutoFlightTurn[],
): AutoFlightArm {
  const replaced = new Set(submissions.map((submission) => submission.taskId));
  return {
    ...arm,
    idleAuthorizedTaskIds: arm.idleAuthorizedTaskIds.filter((taskId) => !replaced.has(taskId)),
    turns: [
      ...arm.turns.filter((turn) => !replaced.has(turn.taskId)),
      ...submissions,
    ].sort(compareTurns),
  };
}

export function reconcileAutoFlightArm(arm: AutoFlightArm, snapshot: MissionSnapshot): AutoFlightArm {
  const running = new Set(
    snapshot.tasks.filter((task) => task.state === "running").map((task) => task.task_id),
  );
  return {
    ...arm,
    idleAuthorizedTaskIds: arm.idleAuthorizedTaskIds.filter((taskId) => running.has(taskId)),
    turns: arm.turns.filter((turn) => running.has(turn.taskId)),
  };
}

export function readAutoFlightArms(value: unknown): AutoFlightArm[] {
  if (!Array.isArray(value)) return [];
  const result: AutoFlightArm[] = [];
  const missions = new Set<string>();
  for (const candidate of value) {
    const arm = readArm(candidate);
    if (!arm || missions.has(arm.missionId)) continue;
    missions.add(arm.missionId);
    result.push(arm);
    if (result.length === MAX_AUTO_FLIGHTS) break;
  }
  return result.sort(compareArms);
}

function readArm(value: unknown): AutoFlightArm | null {
  if (!isRecord(value)
    || !validId(value.missionId)
    || typeof value.missionSha256 !== "string"
    || !/^[a-f0-9]{64}$/u.test(value.missionSha256)
    || !(value.operatorChoiceProvider === null || validId(value.operatorChoiceProvider))
    || !Array.isArray(value.idleAuthorizedTaskIds)
    || value.idleAuthorizedTaskIds.length > MAX_AUTO_FLIGHT_TASKS
    || !value.idleAuthorizedTaskIds.every(validId)
    || !Array.isArray(value.turns)
    || value.turns.length > MAX_AUTO_FLIGHT_TASKS) return null;
  const idle = [...new Set(value.idleAuthorizedTaskIds)].sort();
  const turns = value.turns.map(readTurn);
  if (turns.some((turn) => turn === null)) return null;
  const exactTurns = turns as AutoFlightTurn[];
  const taskIds = new Set(idle);
  for (const turn of exactTurns) {
    if (taskIds.has(turn.taskId)) return null;
    taskIds.add(turn.taskId);
  }
  return {
    missionId: value.missionId,
    missionSha256: value.missionSha256,
    operatorChoiceProvider: value.operatorChoiceProvider,
    idleAuthorizedTaskIds: idle,
    turns: exactTurns.sort(compareTurns),
  };
}

function readTurn(value: unknown): AutoFlightTurn | null {
  return isRecord(value)
    && validId(value.taskId)
    && validId(value.sessionId)
    && Number.isSafeInteger(value.sessionGeneration)
    && (value.sessionGeneration as number) >= 0
    ? {
      taskId: value.taskId,
      sessionId: value.sessionId,
      sessionGeneration: value.sessionGeneration as number,
    }
    : null;
}

function turnMarker(task: MissionTaskLine, session: AutoFlightSession): AutoFlightTurn {
  return {
    taskId: task.task_id,
    sessionId: session.sessionId,
    sessionGeneration: session.sessionGeneration,
  };
}

function validId(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= MAX_ID_LENGTH
    && !/[\u0000-\u001f\u007f]/u.test(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function sameArms(left: readonly AutoFlightArm[], right: readonly AutoFlightArm[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function compareArms(left: AutoFlightArm, right: AutoFlightArm): number {
  return left.missionId.localeCompare(right.missionId);
}

function compareTurns(left: AutoFlightTurn, right: AutoFlightTurn): number {
  return left.taskId.localeCompare(right.taskId);
}
