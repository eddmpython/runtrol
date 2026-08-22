import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import { missionMomentum } from "./momentum";

function task(taskId: string, state: string, sessionId: string | null = null): MissionTaskLine {
  return {
    task_id: taskId,
    key: taskId,
    state,
    instruction_ref: `instructions/${taskId}.md`,
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "operatorChoice",
    output_roots: ["src"],
    artifact_paths: [],
    artifacts: [],
    gate_refs: ["check"],
    capability_versions: [],
    session_id: sessionId,
    workspace: null,
    base_commit: null,
    receipt_id: null,
    run_id: null,
    passed_gates: 0,
    failed_gates: 0,
  };
}

function snapshot(
  state: string,
  tasks: MissionTaskLine[],
  completionPolicy: "allTasks" | "chooseOne" = "allTasks",
): MissionSnapshot {
  return {
    mission: {
      mission_id: "msn_01a02584000070008000000000000001",
      name: "reviewed-change",
      project: "project",
      state,
      completion_policy: completionPolicy,
      passed_tasks: 0,
      total_tasks: tasks.length,
      awaiting_input: tasks.filter((candidate) => candidate.state === "awaitingInput").length,
    },
    mission_sha256: "22".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "33".repeat(32),
    approval_expires_unix_ms: 0,
    tasks,
  };
}

const session = (
  sessionId: string,
  lifecycle: "hotIdle" | "hotRunning" | "cold" | "failed",
  waitingOn: "person" | "quota" | null = null,
) => ({ sessionId, lifecycle, waitingOn });

test("a validated ordinary Mission has one safe start transition", () => {
  const plan = missionMomentum(snapshot("validated", []), []);
  assert.equal(plan.start, true);
  assert.equal(plan.stopped, null);
});

test("choose-one keeps its specialized launch and comparison flow", () => {
  const plan = missionMomentum(snapshot("validated", [], "chooseOne"), []);
  assert.equal(plan.start, false);
  assert.equal(plan.stopped, "specialized mission flow");
});

test("only an exact idle bound task session enters Gate verification", () => {
  const plan = missionMomentum(snapshot("running", [
    task("idle", "running", "s-idle"),
    task("working", "running", "s-working"),
    task("person", "running", "s-person"),
    task("quota", "running", "s-quota"),
    task("failed", "running", "s-failed"),
    task("missing", "running", "s-missing"),
  ]), [
    session("s-idle", "hotIdle"),
    session("s-working", "hotRunning"),
    session("s-person", "hotRunning", "person"),
    session("s-quota", "hotRunning", "quota"),
    session("s-failed", "failed"),
  ]);
  assert.deepEqual(plan.verify.map((candidate) => candidate.task_id), ["idle"]);
  assert.deepEqual(plan.waiting.map((candidate) => candidate.task_id), ["working", "person", "quota"]);
  assert.deepEqual(plan.manual.map((candidate) => candidate.task_id), ["failed", "missing"]);
});

test("the next reviewed wave is prepared and an exact idle binding is sendable", () => {
  const plan = missionMomentum(snapshot("running", [
    task("prepare-first", "reserved"),
    task("send-next", "awaitingInput", "s-next"),
    task("busy", "awaitingInput", "s-busy"),
    task("lost", "awaitingInput", "s-lost"),
  ]), [session("s-next", "hotIdle"), session("s-busy", "hotRunning")]);
  assert.deepEqual(plan.prepare.map((candidate) => candidate.task_id), ["prepare-first"]);
  assert.deepEqual(plan.send.map((candidate) => candidate.task_id), ["send-next"]);
  assert.deepEqual(plan.waiting.map((candidate) => candidate.task_id), ["busy"]);
  assert.deepEqual(plan.manual.map((candidate) => candidate.task_id), ["lost"]);
});

test("an ambiguous provider submission can never be sealed by the convenience path", () => {
  const uncertain = new Set(["uncertain"]);
  const plan = missionMomentum(
    snapshot("running", [task("uncertain", "running", "s-uncertain")]),
    [session("s-uncertain", "hotIdle")],
    uncertain,
  );
  assert.deepEqual(plan.verify, []);
  assert.deepEqual(plan.manual.map((candidate) => candidate.task_id), ["uncertain"]);
});

test("retry, failure, integration, and terminal states stay explicit", () => {
  const running = missionMomentum(snapshot("running", [
    task("retry", "retryable"),
    task("blocked", "blocked"),
    task("failed", "failed"),
  ]), []);
  assert.deepEqual(running.manual.map((candidate) => candidate.task_id), ["retry", "blocked", "failed"]);
  assert.equal(missionMomentum(snapshot("integrating", []), []).stopped, "integrating");
  assert.equal(missionMomentum(snapshot("completed", []), []).stopped, "completed");
});
