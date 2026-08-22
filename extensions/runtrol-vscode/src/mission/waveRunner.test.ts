import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import { MissionWaveRunner, type MissionWavePort } from "./waveRunner";

function task(state: string, sessionId: string | null = null): MissionTaskLine {
  return {
    task_id: "task-one",
    key: "one",
    state,
    instruction_ref: "instructions/one.md",
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "runtime-provider",
    output_roots: ["outputs"],
    artifact_paths: [],
    artifacts: [],
    gate_refs: ["check"],
    capability_versions: [],
    session_id: sessionId,
    workspace: "C:/worktree",
    base_commit: "22".repeat(20),
    receipt_id: null,
    run_id: null,
    passed_gates: 0,
    failed_gates: 0,
  };
}

function snapshot(state: string): MissionSnapshot {
  return {
    mission: {
      mission_id: "mission-one",
      name: "one",
      project: "C:/project",
      state: "running",
      completion_policy: "allTasks",
      passed_tasks: 0,
      total_tasks: 1,
      awaiting_input: state === "awaitingInput" ? 1 : 0,
    },
    mission_sha256: "33".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "44".repeat(32),
    approval_expires_unix_ms: 0,
    tasks: [task(state, state === "reserved" ? null : "session-new")],
  };
}

function fixture(overrides: Partial<MissionWavePort> = {}) {
  const calls: string[] = [];
  let current = snapshot("reserved");
  const port: MissionWavePort = {
    prepare: async (_missionId, _task, provider) => {
      calls.push(`prepare:${provider}`);
      current = snapshot("awaitingInput");
      return { snapshot: current, sessionId: "session-new" };
    },
    hasAmbiguousSubmission: () => false,
    markAmbiguousSubmission: async (taskId) => { calls.push(`mark:${taskId}`); },
    resolveInstruction: async (_missionId, value) => {
      calls.push(`resolve:${value.task_id}`);
      return { sessionId: "session-new", instruction: "exact instruction" };
    },
    submit: async (sessionId) => { calls.push(`submit:${sessionId}`); },
    clearAmbiguousSubmission: async (taskId) => { calls.push(`clear:${taskId}`); },
    getSnapshot: async () => {
      calls.push("get");
      current = snapshot("running");
      return current;
    },
    ...overrides,
  };
  return { calls, runner: new MissionWaveRunner(port) };
}

test("one shared wave prepares, rechecks, sends, and clears exact Task authority", async () => {
  const { calls, runner } = fixture();
  const result = await runner.run(
    snapshot("reserved"),
    [task("reserved")],
    new Map([["task-one", "runtime-provider"]]),
    { report: () => {} },
    true,
  );
  assert.deepEqual(calls, [
    "prepare:runtime-provider",
    "mark:task-one",
    "resolve:task-one",
    "submit:session-new",
    "clear:task-one",
    "get",
  ]);
  assert.deepEqual(result.sessionIds, ["session-new"]);
  assert.equal(result.snapshot.tasks[0]?.state, "running");
});

test("Fleet requires every attempt in the current wave while recovery can leave later eligible work", async () => {
  const { runner } = fixture();
  await assert.rejects(
    runner.run(
      snapshot("eligible"),
      [task("eligible")],
      new Map([["task-one", "runtime-provider"]]),
      { report: () => {} },
      true,
    ),
    /not reserved for this wave/u,
  );
  const result = await runner.run(
    snapshot("eligible"),
    [task("eligible")],
    new Map([["task-one", "runtime-provider"]]),
    { report: () => {} },
    false,
  );
  assert.deepEqual(result.sessionIds, []);
});

test("an ambiguous prior Send is never repeated by a convenience wave", async () => {
  const { calls, runner } = fixture({ hasAmbiguousSubmission: () => true });
  await assert.rejects(
    runner.run(
      snapshot("awaitingInput"),
      [task("awaitingInput", "session-new")],
      new Map([["task-one", "runtime-provider"]]),
      { report: () => {} },
      true,
    ),
    /ambiguous prior Send/u,
  );
  assert.deepEqual(calls, []);
});
