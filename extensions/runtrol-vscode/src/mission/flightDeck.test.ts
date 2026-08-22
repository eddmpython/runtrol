import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import { MAX_FLIGHT_DECK_MISSIONS, missionFlightDeck, type MissionFlightEntry } from "./flightDeck";
import type { MissionMomentum } from "./momentum";

function task(taskId: string): MissionTaskLine {
  return {
    task_id: taskId,
    key: taskId,
    state: "pending",
    instruction_ref: `instructions/${taskId}.md`,
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "operatorChoice",
    output_roots: ["src"],
    artifact_paths: [],
    artifacts: [],
    gate_refs: ["check"],
    capability_versions: [],
    session_id: null,
    workspace: null,
    base_commit: null,
    receipt_id: null,
    run_id: null,
    passed_gates: 0,
    failed_gates: 0,
  };
}

function snapshot(
  id: string,
  state: string,
  completionPolicy: "allTasks" | "chooseOne" = "allTasks",
  expires = 10_000,
  project = `C:/projects/${id}`,
): MissionSnapshot {
  return {
    mission: {
      mission_id: id,
      name: id,
      project,
      state,
      completion_policy: completionPolicy,
      passed_tasks: 0,
      total_tasks: 1,
      awaiting_input: 0,
    },
    mission_sha256: "22".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "33".repeat(32),
    approval_expires_unix_ms: expires,
    tasks: [task(`${id}-task`)],
  };
}

function momentum(overrides: Partial<MissionMomentum>): MissionMomentum {
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

function entry(mission: MissionSnapshot, plan: MissionMomentum): MissionFlightEntry {
  return { snapshot: mission, momentum: plan };
}

test("running safe work is reviewed before starting another Mission", () => {
  const deck = missionFlightDeck([
    entry(snapshot("validated", "validated"), momentum({ start: true })),
    entry(snapshot("send", "running"), momentum({ send: [task("send-task")] })),
    entry(snapshot("verify", "running"), momentum({ verify: [task("verify-task")] })),
  ], 1_000);

  assert.deepEqual(deck.batch.map((candidate) => candidate.snapshot.mission.mission_id), ["send", "verify", "validated"]);
});

test("expired review and ambiguous delivery require individual recovery", () => {
  const deck = missionFlightDeck([
    entry(snapshot("expired", "validated", "allTasks", 999), momentum({ start: true })),
    entry(snapshot("ambiguous", "running"), momentum({ manual: [task("ambiguous-task")] })),
  ], 1_000);

  assert.deepEqual(deck.batch, []);
  assert.deepEqual(deck.manual.map((candidate) => candidate.snapshot.mission.mission_id), ["expired", "ambiguous"]);
});

test("waiting-only, comparison, and integration boundaries never enter the batch", () => {
  const deck = missionFlightDeck([
    entry(snapshot("waiting", "running"), momentum({ waiting: [task("waiting-task")] })),
    entry(snapshot("compare", "validated", "chooseOne"), momentum({ stopped: "specialized mission flow" })),
    entry(snapshot("integrate", "integrating"), momentum({ stopped: "integrating" })),
  ], 1_000);

  assert.deepEqual(deck.waiting.map((candidate) => candidate.snapshot.mission.mission_id), ["waiting"]);
  assert.deepEqual(deck.stopped.map((candidate) => candidate.snapshot.mission.mission_id), ["compare", "integrate"]);
});

test("a waiting Task does not block separate safe work in the same reviewed Mission", () => {
  const deck = missionFlightDeck([
    entry(snapshot("mixed", "running"), momentum({
      waiting: [task("working")],
      prepare: [task("next")],
    })),
  ], 1_000);

  assert.deepEqual(deck.batch.map((candidate) => candidate.snapshot.mission.mission_id), ["mixed"]);
});

test("one confirmation is bounded and additional ready Missions remain visible", () => {
  const candidates = Array.from({ length: MAX_FLIGHT_DECK_MISSIONS + 3 }, (_, index) => {
    const id = `mission-${String(index).padStart(2, "0")}`;
    return entry(snapshot(id, "running"), momentum({ send: [task(`${id}-task`)] }));
  });
  const deck = missionFlightDeck(candidates, 1_000);

  assert.equal(deck.batch.length, MAX_FLIGHT_DECK_MISSIONS);
  assert.equal(deck.remainingReady.length, 3);
  assert.equal(deck.batch.length + deck.remainingReady.length, candidates.length);
});
