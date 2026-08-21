import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { MissionSnapshot } from "../protocol";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

type MomentumResult = {
  snapshot: MissionSnapshot;
  sessionIds: readonly string[];
  verified: number;
};

type JourneyApi = {
  providers(): readonly ProviderLine[];
  registerMissionGate(gateId: string, program: string, arguments_: string[]): Promise<void>;
  validateMissionFile(file: string): Promise<MissionSnapshot>;
  continueMission(missionId: string, operatorChoiceProvider: string): Promise<MomentumResult>;
  close(session: string, now?: boolean): Promise<void>;
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: JourneyApi;
};

const executeFile = promisify(execFile);
let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  const sessions: string[] = [];
  try {
    await eyePass(resultPath, sessions);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        stage: currentStage,
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
      }),
      "utf8",
    );
    throw error;
  } finally {
    const extension = extensionUnderTest<ExtensionApi>();
    const journey = extension.isActive ? extension.exports.journey : undefined;
    if (journey) {
      for (const session of [...sessions]) {
        await journey.close(session, true).catch(() => undefined);
        sessions.splice(sessions.indexOf(session), 1);
      }
    }
  }
}

async function eyePass(resultPath: string, sessions: string[]): Promise<void> {
  const folder = requiredEnvironment("RUNTROL_EYE_FOLDER");
  const preferred = requiredEnvironment("RUNTROL_EYE_PROVIDER");
  await prepareProject(folder);

  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  while (!extension.isActive) await delay(25);
  await within(extension.exports.ready, 60_000, "extension initialization");
  const journey = extension.exports.journey;
  if (!journey) throw new Error("the journey API is unavailable");
  await waitFor(
    () => journey.providers().some((provider) =>
      provider.providerId === preferred && provider.installation.state === "usable"
    ),
    90_000,
    `the installed provider ${preferred}`,
  );

  const missionFile = path.join(folder, "momentum.toml");
  await writeMission(folder, preferred);
  await git(folder, "add", "--", "momentum.toml");
  await git(folder, "commit", "-m", "momentum fixture");

  await journey.registerMissionGate("momentum-eye-check", "git", ["diff", "--check", "HEAD"]);
  const reviewed = await journey.validateMissionFile(missionFile);
  assertMission(reviewed, "validated", ["pending", "pending"]);
  await showMissions();
  await capture(resultPath, "momentumReviewed", {
    state: reviewed.mission.state,
    tasks: reviewed.tasks.map((task) => task.state),
  });

  currentStage = "starting-first-wave";
  const first = await within(
    journey.continueMission(reviewed.mission.mission_id, preferred),
    300_000,
    "first Mission continuation",
  );
  if (first.verified !== 0 || first.sessionIds.length !== 1) {
    throw new Error(`the first continuation verified ${first.verified} and started ${first.sessionIds.length}`);
  }
  sessions.push(...first.sessionIds);
  assertMission(first.snapshot, "running", ["running", "pending"]);
  await journey.waitForLifecycle(first.sessionIds[0], "hotIdle", 240_000);
  await writeTaskArtifact(first.snapshot, "investigate", "first reviewed result\n");

  currentStage = "starting-second-wave";
  const second = await within(
    journey.continueMission(reviewed.mission.mission_id, preferred),
    300_000,
    "second Mission continuation",
  );
  if (second.verified !== 1 || second.sessionIds.length !== 1) {
    throw new Error(`the second continuation verified ${second.verified} and started ${second.sessionIds.length}`);
  }
  sessions.push(...second.sessionIds);
  assertMission(second.snapshot, "running", ["passed", "running"]);
  await showMissions();
  await capture(resultPath, "momentumSecondWave", {
    state: second.snapshot.mission.state,
    tasks: second.snapshot.tasks.map((task) => task.state),
    sessions: sessions.length,
  });

  await journey.waitForLifecycle(second.sessionIds[0], "hotIdle", 240_000);
  await writeTaskArtifact(second.snapshot, "implement", "second reviewed result\n");

  currentStage = "sealing-second-wave";
  const final = await within(
    journey.continueMission(reviewed.mission.mission_id, preferred),
    120_000,
    "final Mission continuation",
  );
  if (final.verified !== 1 || final.sessionIds.length !== 0) {
    throw new Error(`the final continuation verified ${final.verified} and started ${final.sessionIds.length}`);
  }
  assertMission(final.snapshot, "integrating", ["passed", "passed"]);
  await showMissions();
  await capture(resultPath, "momentumIntegration", {
    state: final.snapshot.mission.state,
    tasks: final.snapshot.tasks.map((task) => task.state),
    passed: final.snapshot.mission.passed_tasks,
  });

  currentStage = "closing";
  for (const session of [...sessions]) {
    await journey.close(session, true).catch(() => undefined);
    sessions.splice(sessions.indexOf(session), 1);
  }
  await writeFile(
    resultPath,
    JSON.stringify({ stage: "complete", waves: 2, verified: 2, state: "integrating" }),
    "utf8",
  );
}

async function prepareProject(folder: string): Promise<void> {
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  await mkdir(path.join(folder, "instructions"), { recursive: true });
  await mkdir(path.join(folder, "outputs"), { recursive: true });
  await writeFile(path.join(folder, "instructions", "investigate.md"), "Reply with exactly: done\n", "utf8");
  await writeFile(path.join(folder, "instructions", "implement.md"), "Reply with exactly: done\n", "utf8");
  await writeFile(path.join(folder, "outputs", "investigation.txt"), "base first\n", "utf8");
  await writeFile(path.join(folder, "outputs", "implementation.txt"), "base second\n", "utf8");
  await git(folder, "add", "--", "instructions", "outputs");
  await git(folder, "commit", "-m", "base fixture");
}

async function writeMission(folder: string, provider: string): Promise<void> {
  const first = await digest(path.join(folder, "instructions", "investigate.md"));
  const second = await digest(path.join(folder, "instructions", "implement.md"));
  const mission = `schema = "runtrol.dev/mission/v1alpha1"
name = "mission momentum eye"
project_id = "mission-momentum-eye"
base_ref = "main"
require_clean_base = true
completion_policy = "all_tasks"

[limits]
max_parallel_tasks = 1
max_hot_providers = 1
max_runs_per_task = 1
max_repair_cycles = 0
stop_on_critical_failure = true

[[tasks]]
id = "investigate"
instruction_ref = "instructions/investigate.md"
instruction_sha256 = "${first}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs/investigation.txt"]
gate_refs = ["momentum-eye-check"]

[[tasks]]
id = "implement"
depends_on = ["investigate"]
instruction_ref = "instructions/implement.md"
instruction_sha256 = "${second}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs/implementation.txt"]
gate_refs = ["momentum-eye-check"]
`;
  await writeFile(path.join(folder, "momentum.toml"), mission, "utf8");
}

async function writeTaskArtifact(snapshot: MissionSnapshot, key: string, content: string): Promise<void> {
  const task = snapshot.tasks.find((candidate) => candidate.key === key);
  if (!task?.workspace || task.output_roots.length !== 1) {
    throw new Error(`${key} has no exact prepared Artifact path`);
  }
  await writeFile(path.join(task.workspace, task.output_roots[0]), content, "utf8");
}

function assertMission(snapshot: MissionSnapshot, missionState: string, taskStates: readonly string[]): void {
  if (snapshot.mission.state !== missionState) {
    throw new Error(`the Mission reached ${snapshot.mission.state}, not ${missionState}`);
  }
  const actual = snapshot.tasks.map((task) => task.state);
  if (actual.length !== taskStates.length || actual.some((state, index) => state !== taskStates[index])) {
    throw new Error(`the Task states are ${actual.join(", ")}, not ${taskStates.join(", ")}`);
  }
}

async function digest(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

async function showMissions(): Promise<void> {
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  await vscode.commands.executeCommand("runtrol.missions.focus");
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  await delay(1_200);
}

async function git(folder: string, ...arguments_: string[]): Promise<void> {
  await executeFile("git", arguments_, { cwd: folder, windowsHide: true });
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}`, ...facts }), "utf8");
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await readFile(confirmation, "utf8");
      return;
    } catch {
      if (Date.now() > deadline) throw new Error(`the harness never confirmed the ${pose} capture`);
      await delay(250);
    }
  }
}

async function waitFor(condition: () => boolean, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await delay(100);
  }
}

async function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      Promise.resolve(work),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
