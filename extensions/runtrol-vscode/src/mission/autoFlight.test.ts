import assert from "node:assert/strict";
import test from "node:test";

import type { SessionDescriptor } from "@runtrol/runtime-client";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import {
  MAX_AUTO_FLIGHTS,
  AutoFlights,
  createAutoFlightArm,
  decideAutoFlight,
  readAutoFlightArms,
  recordAutoFlightSubmissions,
  reconcileAutoFlightArm,
} from "./autoFlight";
import type { MissionMomentum } from "./momentum";

function task(key: string, state = "running"): MissionTaskLine {
  return {
    task_id: `task-${key}`,
    key,
    state,
    instruction_ref: `instructions/${key}.md`,
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "operatorChoice",
    output_roots: ["src"],
    artifact_paths: [],
    gate_refs: ["check"],
    capability_versions: [],
    session_id: `session-${key}`,
    workspace: `C:/worktrees/${key}`,
    base_commit: "22".repeat(20),
    receipt_id: null,
    run_id: `run-${key}`,
    passed_gates: 0,
    failed_gates: 0,
  };
}

function snapshot(
  state = "running",
  tasks: MissionTaskLine[] = [task("one")],
  overrides: Partial<MissionSnapshot> = {},
): MissionSnapshot {
  return {
    mission: {
      mission_id: "mission-a",
      name: "Mission A",
      project: "C:/projects/a",
      state,
      completion_policy: "allTasks",
      passed_tasks: 0,
      total_tasks: tasks.length,
      awaiting_input: 0,
    },
    mission_sha256: "ab".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "cd".repeat(32),
    approval_expires_unix_ms: 10_000,
    tasks,
    ...overrides,
  };
}

function session(
  key: string,
  lifecycle: SessionDescriptor["lifecycle"],
  sessionGeneration: number,
  waitingOn: SessionDescriptor["waitingOn"] = null,
): SessionDescriptor {
  return {
    sessionId: `session-${key}`,
    providerId: "runtime-provider",
    workspace: `C:/worktrees/${key}`,
    hot: true,
    lifecycle,
    looksStuck: false,
    waitingOn,
    sessionGeneration,
  };
}

function momentum(overrides: Partial<MissionMomentum> = {}): MissionMomentum {
  return {
    start: false,
    verify: [],
    prepare: [],
    send: [],
    waiting: [],
    manual: [],
    stopped: null,
    ...overrides,
  };
}

test("one arm explicitly authorizes a currently idle reviewed Task", () => {
  const current = snapshot();
  const arm = createAutoFlightArm(
    current,
    "runtime-provider",
    [session("one", "hotIdle", 4)],
    momentum({ verify: current.tasks }),
    1,
  );
  const decision = decideAutoFlight(
    arm,
    current,
    momentum({ verify: current.tasks }),
    [session("one", "hotIdle", 4)],
    2,
  );
  assert.equal(decision.kind, "advance");
  if (decision.kind === "advance") {
    assert.deepEqual(decision.momentum.verify.map((entry) => entry.task_id), ["task-one"]);
  }
});

test("an automatically sent Task needs a real lifecycle generation advance before Gate verification", () => {
  const current = snapshot();
  const reviewed = snapshot("validated", [task("one", "pending")]);
  let arm = createAutoFlightArm(reviewed, "runtime-provider", [], momentum({ start: true }), 1);
  arm = recordAutoFlightSubmissions(arm, [{
    taskId: "task-one",
    sessionId: "session-one",
    sessionGeneration: 7,
  }]);
  const ready = momentum({ verify: current.tasks });
  assert.equal(decideAutoFlight(arm, current, ready, [session("one", "hotIdle", 7)], 2).kind, "wait");
  assert.equal(decideAutoFlight(arm, current, ready, [session("one", "hotRunning", 8)], 2).kind, "wait");
  assert.equal(decideAutoFlight(arm, current, ready, [session("one", "hotIdle", 9)], 2).kind, "advance");
});

test("person and quota waits retain the arm and resume after the exact turn ends", () => {
  for (const waitingOn of ["person", "quota"] as const) {
    const current = snapshot();
    const waiting = session("one", "hotRunning", 8, waitingOn);
    const arm = createAutoFlightArm(current, "runtime-provider", [waiting], momentum({ waiting: current.tasks }), 1);
    assert.equal(decideAutoFlight(arm, current, momentum({ waiting: current.tasks }), [waiting], 2).kind, "wait");
    assert.equal(decideAutoFlight(
      arm,
      current,
      momentum({ verify: current.tasks }),
      [session("one", "hotIdle", 9)],
      3,
    ).kind, "advance");
  }
});

test("pause retains authority and Receipt Landing removes it", () => {
  const current = snapshot();
  const arm = createAutoFlightArm(
    current,
    null,
    [session("one", "hotRunning", 4)],
    momentum({ waiting: current.tasks }),
    1,
  );
  assert.equal(decideAutoFlight(
    arm,
    snapshot("paused"),
    momentum({ stopped: "paused" }),
    [session("one", "hotRunning", 4)],
    2,
  ).kind, "wait");
  assert.equal(decideAutoFlight(
    arm,
    snapshot("integrating", [task("one", "passed")]),
    momentum({ stopped: "integrating" }),
    [],
    3,
  ).kind, "arrived");
});

test("authority drift, expiry, manual recovery, and session replacement disarm", () => {
  const current = snapshot();
  const arm = createAutoFlightArm(
    current,
    null,
    [session("one", "hotIdle", 4)],
    momentum({ verify: current.tasks }),
    1,
  );
  assert.equal(decideAutoFlight(
    arm,
    { ...current, mission_sha256: "ef".repeat(32) },
    momentum(),
    [],
    2,
  ).kind, "disarm");
  assert.equal(decideAutoFlight(arm, current, momentum({ manual: current.tasks }), [], 2).kind, "disarm");

  const sent = recordAutoFlightSubmissions(
    createAutoFlightArm(snapshot("validated", [task("one", "pending")]), null, [], momentum({ start: true }), 1),
    [{ taskId: "task-one", sessionId: "session-one", sessionGeneration: 2 }],
  );
  assert.equal(decideAutoFlight(sent, current, momentum({ verify: current.tasks }), [], 2).kind, "disarm");
  const expired = snapshot("validated", [task("one", "pending")], { approval_expires_unix_ms: 1 });
  assert.equal(decideAutoFlight(sent, expired, momentum({ start: true }), [], 2).kind, "disarm");
});

test("settled Task markers are removed without weakening live turn proof", () => {
  const current = snapshot();
  const arm = {
    ...createAutoFlightArm(
      current,
      null,
      [session("one", "hotIdle", 2)],
      momentum({ verify: current.tasks }),
      1,
    ),
    turns: [{ taskId: "task-two", sessionId: "session-two", sessionGeneration: 3 }],
  };
  const next = reconcileAutoFlightArm(arm, snapshot("running", [task("one"), task("two", "passed")]));
  assert.deepEqual(next.idleAuthorizedTaskIds, ["task-one"]);
  assert.deepEqual(next.turns, []);
});

test("durable updates serialize and a failed write retains conservative authority", async () => {
  const writes: string[][] = [];
  const first = createAutoFlightArm(
    snapshot(),
    null,
    [session("one", "hotIdle", 1)],
    momentum({ verify: snapshot().tasks }),
    1,
  );
  const flights = new AutoFlights([], async (arms) => {
    await Promise.resolve();
    writes.push(arms.map((arm) => arm.missionId));
  });
  await flights.arm(first);
  const second = { ...first, missionId: "mission-b" };
  await Promise.all([flights.arm(second), flights.disarm(first.missionId)]);
  assert.deepEqual(writes, [["mission-a"], ["mission-a", "mission-b"], ["mission-b"]]);

  const failing = new AutoFlights([second], () => Promise.reject(new Error("storage unavailable")));
  await assert.rejects(failing.disarm(second.missionId), /storage unavailable/u);
  assert.equal(failing.isArmed(second.missionId), true);
});

test("several reviewed Missions arm in one durable bounded update", async () => {
  const writes: string[][] = [];
  const first = createAutoFlightArm(
    snapshot(),
    null,
    [session("one", "hotIdle", 1)],
    momentum({ verify: snapshot().tasks }),
    1,
  );
  const flights = new AutoFlights([], (arms) => {
    writes.push(arms.map((arm) => arm.missionId));
    return Promise.resolve();
  });
  await flights.armMany([first, { ...first, missionId: "mission-b" }]);
  assert.deepEqual(writes, [["mission-a", "mission-b"]]);

  await assert.rejects(
    flights.armMany(Array.from({ length: MAX_AUTO_FLIGHTS }, (_unused, index) => ({
      ...first,
      missionId: `extra-${index}`,
    }))),
    /at most 8/u,
  );
  assert.deepEqual(flights.current().map((arm) => arm.missionId), ["mission-a", "mission-b"]);
});

test("restored arms are metadata-only, unique, and bounded to eight Missions", () => {
  const raw = Array.from({ length: MAX_AUTO_FLIGHTS + 3 }, (_unused, index) => ({
    missionId: `mission-${index}`,
    missionSha256: "ab".repeat(32),
    operatorChoiceProvider: null,
    idleAuthorizedTaskIds: [],
    turns: [],
  }));
  raw.unshift({
    missionId: "bad",
    missionSha256: "prompt body",
    operatorChoiceProvider: null,
    idleAuthorizedTaskIds: [],
    turns: [],
  });
  const restored = readAutoFlightArms(raw);
  assert.equal(restored.length, MAX_AUTO_FLIGHTS);
  assert.equal(restored.some((arm) => arm.missionId === "bad"), false);
});
