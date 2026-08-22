import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import {
  assertInterruptedRecoveryAuthority,
  interruptedRecoveryDetail,
  interruptedRecoveryPlan,
  recoveryTasks,
} from "./recovery";

function task(key: string, state: string): MissionTaskLine {
  return {
    task_id: `task-${key}`,
    key,
    state,
    instruction_ref: `instructions/${key}.md`,
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "operatorChoice",
    output_roots: ["outputs"],
    artifact_paths: [],
    artifacts: [],
    gate_refs: ["check"],
    capability_versions: [],
    session_id: null,
    workspace: `C:/worktrees/${key}`,
    base_commit: "22".repeat(20),
    receipt_id: null,
    run_id: null,
    passed_gates: 0,
    failed_gates: 0,
  };
}

function snapshot(
  completionPolicy: MissionSnapshot["mission"]["completion_policy"] = "allTasks",
  tasks: MissionTaskLine[] = [task("ambiguous", "blocked")],
): MissionSnapshot {
  return {
    mission: {
      mission_id: "mission-recovery",
      name: "Interrupted work",
      project: "C:/project",
      state: "blocked",
      completion_policy: completionPolicy,
      passed_tasks: 0,
      total_tasks: tasks.length,
      awaiting_input: 0,
    },
    mission_sha256: "33".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "44".repeat(32),
    approval_expires_unix_ms: 0,
    tasks,
  };
}

test("ordinary and choose-one Missions freeze the same exact interrupted recovery authority", () => {
  for (const policy of ["allTasks", "chooseOne"] as const) {
    const plan = interruptedRecoveryPlan(snapshot(policy));
    assert.equal(plan.completionPolicy, policy);
    assert.deepEqual(plan.tasks.map((candidate) => candidate.taskId), ["task-ambiguous"]);
    assert.doesNotThrow(() => assertInterruptedRecoveryAuthority(plan, snapshot(policy)));
  }
});

test("contract or workspace loss refuses automatic recovery", () => {
  assert.throws(
    () => interruptedRecoveryPlan(snapshot("unavailableAfterRestart")),
    /contract changed after restart/u,
  );
  assert.throws(
    () => interruptedRecoveryPlan(snapshot("allTasks", [{
      ...task("lost", "blocked"),
      workspace_mode: "unavailableAfterRestart",
      workspace: null,
      base_commit: null,
    }])),
    /lost its reviewed workspace authority/u,
  );
});

test("a second interruption includes already reopened work without widening to pending work", () => {
  const current = snapshot("allTasks", [
    task("reopened", "eligible"),
    task("ambiguous", "blocked"),
    task("reserved", "reserved"),
    task("later", "pending"),
    task("done", "passed"),
  ]);
  const plan = interruptedRecoveryPlan(current);
  assert.deepEqual(plan.tasks.map((candidate) => candidate.key), ["reopened", "ambiguous", "reserved"]);
  assert.deepEqual(recoveryTasks(plan, current).map((candidate) => candidate.key), ["reopened", "ambiguous", "reserved"]);

  const betweenRetryAndResume = snapshot("allTasks", [task("reopened", "eligible")]);
  assert.deepEqual(
    interruptedRecoveryPlan(betweenRetryAndResume).tasks.map((candidate) => candidate.key),
    ["reopened"],
  );
});

test("digest, policy, task, provider, and workspace drift all revoke confirmation", () => {
  const current = snapshot();
  const plan = interruptedRecoveryPlan(current);
  const changes: MissionSnapshot[] = [
    { ...current, mission_sha256: "55".repeat(32) },
    { ...current, policy_sha256: "66".repeat(32) },
    { ...current, tasks: [{ ...current.tasks[0], provider_selector: "runtime:other" }] },
    { ...current, tasks: [{ ...current.tasks[0], workspace: "C:/other" }] },
    { ...current, tasks: [{ ...current.tasks[0], state: "eligible" }] },
  ];
  for (const changed of changes) {
    assert.throws(
      () => assertInterruptedRecoveryAuthority(plan, changed),
      /changed after review|no interrupted Task/u,
    );
  }
});

test("the confirmation names exact identities and the duplicate-effect risk", () => {
  const plan = interruptedRecoveryPlan(snapshot());
  const detail = interruptedRecoveryDetail(plan, new Map([["task-ambiguous", "runtime-provider"]]));
  assert.match(detail, /Mission SHA-256 3333/u);
  assert.match(detail, /Policy SHA-256 4444/u);
  assert.match(detail, /ambiguous: runtime-provider/u);
  assert.match(detail, /C:\/worktrees\/ambiguous/u);
  assert.match(detail, /may already have caused external effects/u);
  assert.match(detail, /may repeat those effects/u);
});
