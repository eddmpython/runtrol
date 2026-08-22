import assert from "node:assert/strict";
import test from "node:test";

import type { MissionSnapshot, MissionTaskLine } from "../../protocol";
import {
  landingByteDriftProblem,
  landingCompletionProblem,
  landingIdentity,
  missionLanding,
  missionLandingAuthority,
  missionLandingForSelection,
  missionLandingQueue,
  missionWinnerLanding,
  safeArtifactPath,
} from "./model";

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
    artifacts: artifactPaths.map((path) => ({ path, size: 1, sha256: "55".repeat(32) })),
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

test("one Fleet winner review contains only the exact selected passing Receipt", () => {
  const race = snapshot("race", [
    task("one", ["src/main.ts", "src/one.ts"]),
    task("two", ["src/main.ts", "src/two.ts"]),
  ], "chooseOne");
  const winner = missionWinnerLanding(race, "task-two");

  assert.ok(winner);
  assert.deepEqual(winner.selection, { kind: "chooseOne", taskId: "task-two" });
  assert.deepEqual(winner.artifacts.map((artifact) => artifact.path), ["src/main.ts", "src/two.ts"]);
  assert.ok(winner.artifacts.every((artifact) => artifact.task.task_id === "task-two"));
  assert.equal(missionWinnerLanding(race, "task-missing"), null);
  race.tasks[1].state = "failed";
  assert.equal(missionWinnerLanding(race, "task-two"), null);
});

test("Fleet winner authority rejects policy and selection drift", () => {
  const race = snapshot("race", [
    task("one", ["src/main.ts"]),
    task("two", ["src/main.ts"]),
  ], "chooseOne");
  const first = missionWinnerLanding(race, "task-one");
  const second = missionWinnerLanding(race, "task-two");

  assert.ok(first);
  assert.ok(second);
  assert.notEqual(landingIdentity(first), landingIdentity(second));
  assert.equal(missionLandingAuthority(race), null);
  assert.equal(
    missionLandingAuthority(snapshot("ordinary", [task("one", ["src/main.ts"])]), {
      kind: "chooseOne",
      taskId: "task-one",
    }),
    null,
  );
});

test("missing durable evidence refuses a review that could falsely authorize completion", () => {
  const oldCoreTask = task("old-core", ["src/a.ts"]);
  delete oldCoreTask.artifacts;
  for (const broken of [
    oldCoreTask,
    task("workspace", ["src/a.ts"], { workspace: null }),
    task("receipt", ["src/a.ts"], { receipt_id: null }),
    task("artifact", []),
    task("metadata", ["src/a.ts"], { artifacts: [] }),
    task("digest", ["src/a.ts"], { artifacts: [{ path: "src/a.ts", size: 1, sha256: "invalid" }] }),
  ]) {
    assert.equal(missionLanding(snapshot(broken.key, [broken])), null);
  }
});

test("unsafe and overlapping target paths are never hidden inside the combined review", () => {
  assert.equal(safeArtifactPath("src/main.ts"), true);
  for (const value of ["../secret", "/root/file", "C:/outside", "src\\file", "src//file", "src/./file"]) {
    assert.equal(safeArtifactPath(value), false, value);
  }
  assert.equal(missionLanding(snapshot("overlap", [
    task("one", ["src/main.ts"]),
    task("two", ["src/main.ts"]),
  ])), null);
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

test("Landing identity changes with Mission, Receipt, workspace, and target authority", () => {
  const original = missionLanding(snapshot("ship", [task("one", ["src/main.ts"])]));
  assert.ok(original);
  for (const changed of [
    snapshot("ship", [task("one", ["src/main.ts"], { receipt_id: "rcp_changed" })]),
    snapshot("ship", [task("one", ["src/main.ts"], { workspace: "C:/worktrees/changed" })]),
    snapshot("ship", [task("one", ["src/other.ts"])]),
  ]) {
    const landing = missionLanding(changed);
    assert.ok(landing);
    assert.notEqual(landingIdentity(landing), landingIdentity(original));
  }
});

test("Landing identity survives only the expected integrating-to-completed lifecycle transition", () => {
  const integrating = missionLanding(snapshot("ship", [task("one", ["src/main.ts"])]));
  const completedSnapshot = snapshot("ship", [task("one", ["src/main.ts"])]);
  completedSnapshot.mission.state = "completed";
  const completed = missionLandingAuthority(completedSnapshot);

  assert.ok(integrating);
  assert.ok(completed);
  assert.equal(landingIdentity(completed), landingIdentity(integrating));
  assert.equal(missionLanding(completedSnapshot), null);
});

test("winner authority survives completion but a new apply review cannot start after completion", () => {
  assert.equal(missionWinnerLanding(
    snapshot("winner", [task("one", ["src/main.ts"])], "chooseOne"),
    "task-one",
  )?.selection.kind, "chooseOne");
  const completedWinner = snapshot("winner", [task("one", ["src/main.ts"])], "chooseOne");
  completedWinner.mission.state = "completed";
  assert.equal(missionWinnerLanding(completedWinner, "task-one"), null);
  assert.equal(
    missionLandingForSelection(completedWinner, { kind: "chooseOne", taskId: "task-one" }),
    null,
  );
  assert.ok(missionLandingAuthority(completedWinner, { kind: "chooseOne", taskId: "task-one" }));
});

test("completed winner authority names the exact Task and Receipt even when candidate bytes match", () => {
  const completed = snapshot("race", [
    task("one", ["src/main.ts"]),
    task("two", ["src/main.ts"]),
  ], "chooseOne");
  completed.mission.state = "completed";
  completed.integration = {
    selected_task_id: "task-two",
    selected_receipt_id: "rcp_two",
  };
  const second = missionLandingAuthority(completed, { kind: "chooseOne", taskId: "task-two" });
  assert.ok(second);
  assert.equal(landingCompletionProblem(second), null);

  completed.integration = {
    selected_task_id: "task-one",
    selected_receipt_id: "rcp_one",
  };
  const staleSecond = missionLandingAuthority(completed, { kind: "chooseOne", taskId: "task-two" });
  assert.ok(staleSecond);
  assert.match(landingCompletionProblem(staleSecond) ?? "", /different selected Task Receipt/);
});

test("source, target, existence, and Artifact-set drift are named before apply", () => {
  const reviewed = [{
    path: "src/main.ts",
    sourceBytes: Uint8Array.of(1, 2),
    targetBytes: Uint8Array.of(3),
  }];
  assert.equal(landingByteDriftProblem(reviewed, reviewed), null);
  assert.match(landingByteDriftProblem(reviewed, [{ ...reviewed[0], sourceBytes: Uint8Array.of(2) }]) ?? "", /Receipt/);
  assert.match(landingByteDriftProblem(reviewed, [{ ...reviewed[0], targetBytes: Uint8Array.of(4) }]) ?? "", /Project/);
  assert.match(landingByteDriftProblem(reviewed, [{ ...reviewed[0], targetBytes: null }]) ?? "", /existence/);
  assert.match(landingByteDriftProblem(reviewed, []) ?? "", /set/);
});
