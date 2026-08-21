import type { SessionDescriptor } from "@runtrol/runtime-client";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";

type MomentumSession = Pick<SessionDescriptor, "sessionId" | "lifecycle" | "waitingOn">;

export type MissionMomentum = {
  readonly start: boolean;
  readonly verify: readonly MissionTaskLine[];
  readonly prepare: readonly MissionTaskLine[];
  readonly send: readonly MissionTaskLine[];
  readonly waiting: readonly MissionTaskLine[];
  readonly manual: readonly MissionTaskLine[];
  readonly stopped: string | null;
};

/// Every transition the local surface can prove safe from one current snapshot.
///
/// This reads only scheduler state and bounded Runtime metadata. Provider output, events, artifacts, and instruction
/// text are deliberately absent, so adding this convenience cannot become another agent loop.
export function missionMomentum(
  snapshot: MissionSnapshot,
  sessions: readonly MomentumSession[],
  ambiguousSubmissions: ReadonlySet<string> = new Set(),
): MissionMomentum {
  if (snapshot.mission.completion_policy !== "allTasks") {
    return stoppedMomentum("specialized mission flow");
  }
  if (snapshot.mission.state === "validated") {
    return {
      start: true,
      verify: [],
      prepare: [],
      send: [],
      waiting: [],
      manual: [],
      stopped: null,
    };
  }
  if (snapshot.mission.state !== "running") {
    return stoppedMomentum(snapshot.mission.state);
  }

  const sessionsById = new Map(sessions.map((session) => [session.sessionId, session]));
  const verify: MissionTaskLine[] = [];
  const prepare: MissionTaskLine[] = [];
  const send: MissionTaskLine[] = [];
  const waiting: MissionTaskLine[] = [];
  const manual: MissionTaskLine[] = [];

  for (const task of snapshot.tasks) {
    if (ambiguousSubmissions.has(task.task_id)) {
      manual.push(task);
      continue;
    }
    if (task.state === "reserved") {
      prepare.push(task);
      continue;
    }
    if (task.state === "awaitingInput") {
      classifyAwaitingInput(task, sessionsById, send, waiting, manual);
      continue;
    }
    if (task.state === "awaitingApproval") {
      waiting.push(task);
      continue;
    }
    if (task.state !== "running") {
      if (task.state === "retryable" || task.state === "blocked" || task.state === "failed") {
        manual.push(task);
      }
      continue;
    }
    const session = task.session_id ? sessionsById.get(task.session_id) : null;
    if (!session) {
      manual.push(task);
    } else if (session.waitingOn !== null && session.waitingOn !== undefined) {
      waiting.push(task);
    } else if (session.lifecycle === "hotIdle") {
      verify.push(task);
    } else if (session.lifecycle === "hotRunning") {
      waiting.push(task);
    } else {
      manual.push(task);
    }
  }

  return { start: false, verify, prepare, send, waiting, manual, stopped: null };
}

export function hasMissionMomentumWork(momentum: MissionMomentum): boolean {
  return momentum.start
    || momentum.verify.length > 0
    || momentum.prepare.length > 0
    || momentum.send.length > 0;
}

function classifyAwaitingInput(
  task: MissionTaskLine,
  sessionsById: ReadonlyMap<string, MomentumSession>,
  send: MissionTaskLine[],
  waiting: MissionTaskLine[],
  manual: MissionTaskLine[],
): void {
  const session = task.session_id ? sessionsById.get(task.session_id) : null;
  if (!session) {
    manual.push(task);
  } else if (
    (session.waitingOn !== null && session.waitingOn !== undefined)
    || session.lifecycle === "hotRunning"
  ) {
    waiting.push(task);
  } else if (session.lifecycle === "hotIdle") {
    send.push(task);
  } else {
    manual.push(task);
  }
}

function stoppedMomentum(reason: string): MissionMomentum {
  return {
    start: false,
    verify: [],
    prepare: [],
    send: [],
    waiting: [],
    manual: [],
    stopped: reason,
  };
}
