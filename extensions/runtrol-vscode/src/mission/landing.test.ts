import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../protocol";
import { missionLanding, missionLandingQueue, safeArtifactPath } from "./landing";

function task(
  key: string,
  artifactPaths: string[],
  overrides: Partial<MissionTaskLine> = {},
): MissionTaskLine {
  return {
    task_id: `task-${key}`,
    key,
    state: "passed",
    instruction_ref: `instructions/${key}.md`,
    instruction_sha256: "11".repeat(32),
    workspace_mode: "isolatedWorktree",
    provider_selector: "operatorChoice",
    output_roots: ["src"],
    artifact_paths: artifactPaths,
    gate_refs: ["check"],
    capability_versions: [],
    session_id: `session-${key}`,
    workspace: `C:/worktrees/${key}`,
    base_commit: "22".repeat(20),
    receipt_id: `rcp_${key}`,
    run_id: `run-${key}`,
    passed_gates: 2,
    failed_gates: 0,
    ...overrides,
  };
}

function snapshot(
  id: string,
  tasks: MissionTaskLine[],
  completionPolicy: "allTasks" | "chooseOne" = "allTasks",
): MissionSnapshot {
  return {
    mission: {
      mission_id: id,
      name: id,
      project: `C:/projects/${id}`,
      state: "integrating",
      completion_policy: completionPolicy,
      passed_tasks: tasks.length,
      total_tasks: tasks.length,
      awaiting_input: 0,
    },
    mission_sha256: "33".repeat(32),
    mission_ref: "mission.toml",
    policy_sha256: "44".repeat(32),
    approval_expires_unix_ms: 10_000,
    tasks,
  };
}

test("one landing combines every passed Task Receipt into one sorted target review", () => {
  const landing = missionLanding(snapshot("ship", [
    task("docs", ["docs/guide.md"]),
    task("code", ["src/z.ts", "src/a.ts"]),
  ]));

  assert.ok(landing);
  assert.deepEqual(landing.artifacts.map((artifact) => artifact.path), [
    "docs/guide.md",
    "src/a.ts",
    "src/z.ts",
  ]);
});

test("comparison Missions stay on their explicit winner-selection path", () => {
  assert.equal(missionLanding(snapshot("race", [task("one", ["src/main.ts"])], "chooseOne")), null);
});

test("missing durable evidence refuses a review that could falsely authorize completion", () => {
  for (const broken of [
    task("workspace", ["src/a.ts"], { workspace: null }),
    task("receipt", ["src/a.ts"], { receipt_id: null }),
    task("artifact", []),
  ]) {
    assert.equal(missionLanding(snapshot(broken.key, [broken])), null);
  }
});

test("unsafe and overlapping target paths are never hidden inside the combined review", () => {
  assert.equal(safeArtifactPath("src/main.ts"), true);
  for (const value of ["../secret", "/root/file", "C:/outside", "src\\file", "src//file", "src/./file"]) {
    assert.equal(safeArtifactPath(value), false, value);
  }
  const overlap = missionLanding(snapshot("overlap", [
    task("one", ["src/main.ts"]),
    task("two", ["src/main.ts"]),
  ]));
  assert.equal(overlap, null);
  assert.equal(missionLanding(snapshot("case-overlap", [
    task("one", ["src/Main.ts"]),
    task("two", ["src/main.ts"]),
  ])), null);
});

test("one review never outgrows the bounded two-sided document cache", () => {
  const tasks = Array.from({ length: 5 }, (_unused, taskIndex) => task(
    `batch-${taskIndex}`,
    Array.from({ length: taskIndex === 4 ? 204 : 205 }, (_inner, artifactIndex) =>
      `src/${taskIndex}/${artifactIndex}.ts`
    ),
  ));
  assert.equal(missionLanding(snapshot("bounded", tasks))?.artifacts.length, 1_024);

  tasks[4] = task(
    "batch-4",
    Array.from({ length: 205 }, (_unused, artifactIndex) => `src/4/${artifactIndex}.ts`),
  );
  assert.equal(missionLanding(snapshot("too-large", tasks)), null);
});

test("the cross-project queue is deterministic and exposes every excluded Mission", () => {
  const queue = missionLandingQueue([
    snapshot("zeta", [task("zeta", ["z.txt"])]),
    snapshot("compare", [task("compare", ["c.txt"])], "chooseOne"),
    snapshot("broken", [task("broken", ["b.txt"], { workspace: null })]),
    snapshot("alpha", [task("alpha", ["a.txt"])]),
  ]);

  assert.deepEqual(queue.map((landing) => landing.snapshot.mission.mission_id), ["alpha", "zeta"]);
});
