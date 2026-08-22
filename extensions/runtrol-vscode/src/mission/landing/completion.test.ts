import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot } from "../../protocol";
import { completeLandingWithRecovery } from "./completion";

function snapshot(state: string): MissionSnapshot {
  return {
    mission: {
      mission_id: "mission-1",
      name: "Mission 1",
      project: "C:/project",
      state,
      completion_policy: "allTasks",
      passed_tasks: 1,
      total_tasks: 1,
      awaiting_input: 0,
    },
    mission_sha256: "11".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "22".repeat(32),
    approval_expires_unix_ms: 1,
    tasks: [],
  };
}

test("a lost completion response converges from observed Core authority without completing twice", async () => {
  let completeCalls = 0;
  let validateCalls = 0;
  const result = await completeLandingWithRecovery(
    snapshot("integrating"),
    async () => {
      completeCalls += 1;
      throw new Error("response lost");
    },
    async () => snapshot("completed"),
    async (completed) => {
      validateCalls += 1;
      assert.equal(completed.mission.state, "completed");
    },
  );

  assert.equal(result.mission.state, "completed");
  assert.equal(completeCalls, 1);
  assert.equal(validateCalls, 1);
});

test("a retry that already observes completion validates authority without another Core call", async () => {
  let completeCalls = 0;
  const result = await completeLandingWithRecovery(
    snapshot("completed"),
    async () => {
      completeCalls += 1;
      return snapshot("completed");
    },
    async () => {
      throw new Error("refresh must not run");
    },
    async () => undefined,
  );

  assert.equal(result.mission.state, "completed");
  assert.equal(completeCalls, 0);
});

test("an ambiguous failure stays failed when Core did not reach completed", async () => {
  await assert.rejects(
    completeLandingWithRecovery(
      snapshot("integrating"),
      async () => {
        throw new Error("response lost");
      },
      async () => snapshot("integrating"),
      async () => undefined,
    ),
    /response lost/,
  );
});
