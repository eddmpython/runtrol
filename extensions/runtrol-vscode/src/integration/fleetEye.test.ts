import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { MissionSnapshot } from "../protocol";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  registerMissionGate(gateId: string, program: string, arguments_: string[]): Promise<void>;
  validateMissionFile(file: string): Promise<MissionSnapshot>;
  launchFleet(missionId: string): Promise<string[]>;
  mission(missionId: string): Promise<MissionSnapshot>;
  verifyMissionTask(missionId: string, taskId: string): Promise<MissionSnapshot>;
  compareMissionResults(missionId: string): Promise<void>;
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
  const second = process.env.RUNTROL_FLEET_EYE_SECOND_PROVIDER ?? preferred;
  const missionFile = path.join(folder, "fleet.toml");
  await writeMission(folder, [preferred, second]);
  await git(folder, "add", "--", "fleet.toml");
  await git(folder, "commit", "-m", "fleet fixture");

  await journey.registerMissionGate("fleet-eye-check", "git", ["diff", "--check", "HEAD"]);
  const reviewed = await journey.validateMissionFile(missionFile);
  await showMissions();
  await capture(resultPath, "fleetReviewed", {
    policy: reviewed.mission.completion_policy,
    tasks: reviewed.tasks.length,
    providers: [preferred, second],
  });

  currentStage = "launching";
  sessions.push(...await within(journey.launchFleet(reviewed.mission.mission_id), 300_000, "fleet launch"));
  for (const session of sessions) {
    await journey.waitForLifecycle(session, "hotIdle", 240_000);
  }
  await delay(1_500);
  await capture(resultPath, "fleetGrid", {
    sessions: sessions.length,
    groups: vscode.window.tabGroups.all.length,
  });

  currentStage = "sealing-results";
  let snapshot = await journey.mission(reviewed.mission.mission_id);
  for (const [index, task] of snapshot.tasks.entries()) {
    if (!task.workspace) throw new Error(`${task.key} has no prepared worktree`);
    await mkdir(path.join(task.workspace, "outputs"), { recursive: true });
    await writeFile(path.join(task.workspace, "outputs", "result.txt"), `attempt ${index + 1}\n`, "utf8");
    snapshot = await within(
      journey.verifyMissionTask(snapshot.mission.mission_id, task.task_id),
      90_000,
      `verifying ${task.key}`,
    );
  }
  if (snapshot.mission.state !== "integrating") {
    throw new Error(`the comparison Mission reached ${snapshot.mission.state}, not integration review`);
  }
  await showMissions();
  await capture(resultPath, "fleetResults", {
    state: snapshot.mission.state,
    passed: snapshot.mission.passed_tasks,
    artifacts: snapshot.tasks.map((task) => task.artifact_paths),
  });

  currentStage = "comparing";
  await journey.compareMissionResults(snapshot.mission.mission_id);
  await delay(1_500);
  await capture(resultPath, "fleetDiff", {
    editors: vscode.window.tabGroups.all.flatMap((group) => group.tabs).length,
    groups: vscode.window.tabGroups.all.length,
  });

  currentStage = "closing";
  for (const session of [...sessions]) {
    await journey.close(session, true).catch(() => undefined);
    sessions.splice(sessions.indexOf(session), 1);
  }
  await writeFile(
    resultPath,
    JSON.stringify({ stage: "complete", policy: "chooseOne", attempts: 2, diff: true }),
    "utf8",
  );
}

async function prepareProject(folder: string): Promise<void> {
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  await mkdir(path.join(folder, "instructions"), { recursive: true });
  await mkdir(path.join(folder, "outputs"), { recursive: true });
  await writeFile(path.join(folder, "instructions", "compare.md"), "Reply with exactly: done\n", "utf8");
  await writeFile(path.join(folder, "outputs", "result.txt"), "base\n", "utf8");
  await git(folder, "add", "--", "instructions", "outputs");
  await git(folder, "commit", "-m", "base fixture");
}

async function writeMission(folder: string, providers: readonly string[]): Promise<void> {
  const instruction = await readFile(path.join(folder, "instructions", "compare.md"));
  const digest = createHash("sha256").update(instruction).digest("hex");
  const tasks = providers.map((provider, index) => `
[[tasks]]
id = "attempt-${index + 1}"
instruction_ref = "instructions/compare.md"
instruction_sha256 = "${digest}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs/result.txt"]
gate_refs = ["fleet-eye-check"]
`).join("");
  const mission = `schema = "runtrol.dev/mission/v1alpha1"
name = "fleet eye"
project_id = "fleet-eye"
base_ref = "main"
require_clean_base = true
completion_policy = "choose_one"

[limits]
max_parallel_tasks = 2
max_hot_providers = 2
max_runs_per_task = 1
max_repair_cycles = 0
stop_on_critical_failure = false
${tasks}`;
  await writeFile(path.join(folder, "fleet.toml"), mission, "utf8");
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
