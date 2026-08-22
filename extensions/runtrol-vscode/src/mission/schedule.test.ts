import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot } from "../protocol";
import {
  assertMissionScheduleAuthority,
  localScheduleInput,
  parseLocalScheduleInput,
  reviewMissionSchedule,
  tomorrowAtNine,
} from "./schedule";

function snapshot(): MissionSnapshot {
  return {
    mission: {
      mission_id: "mission-one",
      name: "Nightly review",
      project: "C:/work/project",
      state: "validated",
      completion_policy: "allTasks",
      passed_tasks: 0,
      total_tasks: 1,
      awaiting_input: 0,
      schedule: null,
    },
    mission_sha256: "mission-sha",
    mission_ref: "mission.toml",
    policy_sha256: "policy-sha",
    approval_expires_unix_ms: 0,
    integration: null,
    tasks: [{
      task_id: "task-one",
      key: "inspect",
      state: "pending",
      instruction_ref: "instructions/inspect.md",
      instruction_sha256: "instruction-sha",
      workspace_mode: "readOnlyBase",
      provider_selector: "operatorChoice",
      output_roots: [],
      artifact_paths: [],
      artifacts: [],
      gate_refs: [],
      capability_versions: [],
      session_id: null,
      workspace: null,
      base_commit: null,
      receipt_id: null,
      run_id: null,
      passed_gates: 0,
      failed_gates: 0,
    }],
  };
}

test("schedule review freezes due time, replacement CAS, Task and provider authority", () => {
  const current = snapshot();
  current.mission.schedule = {
    schedule_id: "sch_previous",
    due_unix_ms: 20_000,
    state: "pending",
    providers: [{ task_id: "task-one", provider_runtime_id: "runtime-a" }],
    failure: null,
  };
  const review = reviewMissionSchedule(
    current,
    "sch_018f0000-0000-7000-8000-000000000000",
    30_000,
    new Map([["task-one", "runtime-a"]]),
    10_000,
  );
  assert.equal(review.replacesScheduleId, "sch_previous");
  assert.deepEqual(review.providers, [{ task_id: "task-one", provider_runtime_id: "runtime-a" }]);
  assert.doesNotThrow(() => assertMissionScheduleAuthority(review, current));

  const changed = structuredClone(current);
  changed.tasks[0].instruction_sha256 = "changed";
  assert.throws(() => assertMissionScheduleAuthority(review, changed), /authority changed/u);
  const replaced = structuredClone(current);
  replaced.mission.schedule!.schedule_id = "sch_other";
  assert.throws(() => assertMissionScheduleAuthority(review, replaced), /authority changed/u);
});

test("local schedule input is strict and tomorrow means local 09:00", () => {
  const local = new Date(2030, 4, 6, 14, 35, 0, 0).getTime();
  assert.equal(parseLocalScheduleInput(localScheduleInput(local)), local);
  assert.equal(parseLocalScheduleInput("2030-02-30 09:00"), null);
  assert.equal(parseLocalScheduleInput("2030-05-06T09:00Z"), null);
  const tomorrow = new Date(tomorrowAtNine(local));
  assert.equal(tomorrow.getDate(), 7);
  assert.equal(tomorrow.getHours(), 9);
  assert.equal(tomorrow.getMinutes(), 0);
});

test("schedule refuses omissions, foreign Tasks, and unsafe lead times", () => {
  const current = snapshot();
  assert.throws(
    () => reviewMissionSchedule(current, "sch_valid", 30_000, new Map(), 10_000),
    /no reviewed provider/u,
  );
  assert.throws(
    () => reviewMissionSchedule(
      current,
      "sch_valid",
      30_000,
      new Map([["task-one", "runtime-a"], ["foreign", "runtime-b"]]),
      10_000,
    ),
    /outside/u,
  );
  assert.throws(
    () => reviewMissionSchedule(current, "sch_valid", 10_500, new Map([["task-one", "runtime-a"]]), 10_000),
    /between one second/u,
  );
});
