import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, rename, rmdir, symlink, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { MissionSnapshot } from "../protocol";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

type FlightDeckResult = {
  missions: number;
  sessionIds: readonly string[];
  verified: number;
  remainingReady: number;
};

type JourneyApi = {
  providers(): readonly ProviderLine[];
  registerMissionGate(gateId: string, program: string, arguments_: string[]): Promise<void>;
  validateMissionFile(file: string): Promise<MissionSnapshot>;
  continueReadyMissions(operatorChoiceProvider: string): Promise<FlightDeckResult>;
  mission(missionId: string): Promise<MissionSnapshot>;
  reviewMissionLanding(missionId: string): Promise<void>;
  applyMissionLanding(missionId: string): Promise<MissionSnapshot>;
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
  const root = requiredEnvironment("RUNTROL_EYE_FOLDER");
  const preferred = requiredEnvironment("RUNTROL_EYE_PROVIDER");
  const projects = [
    { name: "Flight Alpha", id: "flight-alpha", folder: path.join(root, "flight-alpha") },
    { name: "Flight Beta", id: "flight-beta", folder: path.join(root, "flight-beta") },
  ];
  await Promise.all(projects.map((project) => prepareProject(project.folder, project.name, project.id, preferred)));
  const existing = vscode.workspace.workspaceFolders?.length ?? 0;
  if (!vscode.workspace.updateWorkspaceFolders(
    0,
    existing,
    ...projects.map((project) => ({ uri: vscode.Uri.file(project.folder), name: project.name })),
  )) {
    throw new Error("the eye workspace could not expose both Mission projects");
  }
  await waitFor(
    () => vscode.workspace.workspaceFolders?.length === projects.length,
    10_000,
    "both Mission workspace folders",
  );

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

  await journey.registerMissionGate("flight-deck-eye-check", "git", ["diff", "--check", "HEAD"]);
  await journey.registerMissionGate("flight-deck-eye-mutates-once", requiredEnvironment("RUNTROL_EYE_NODE"), [
    "gate.cjs",
  ]);
  const reviewed: MissionSnapshot[] = [];
  for (const project of projects) {
    const snapshot = await journey.validateMissionFile(path.join(project.folder, "mission.toml"));
    assertMission(snapshot, "validated", ["pending"]);
    reviewed.push(snapshot);
  }
  await showMissions();
  await capture(resultPath, "flightDeckReviewed", {
    missions: reviewed.map((snapshot) => snapshot.mission.name),
    states: reviewed.map((snapshot) => snapshot.mission.state),
  });

  currentStage = "starting-flight";
  const first = await within(
    journey.continueReadyMissions(preferred),
    300_000,
    "first Mission Flight Deck continuation",
  );
  if (first.missions !== 2 || first.verified !== 0 || first.sessionIds.length !== 2 || first.remainingReady !== 0) {
    throw new Error(
      `the first flight advanced ${first.missions}, verified ${first.verified}, started ${first.sessionIds.length}, and left ${first.remainingReady}`,
    );
  }
  sessions.push(...first.sessionIds);
  await Promise.all(first.sessionIds.map((session) => journey.waitForLifecycle(session, "hotIdle", 240_000)));
  const running = await Promise.all(reviewed.map((snapshot) => journey.mission(snapshot.mission.mission_id)));
  for (const snapshot of running) {
    assertMission(snapshot, "running", ["running"]);
    await writeTaskArtifact(snapshot, `${snapshot.mission.name} reviewed result\n`);
  }
  await showMissions();
  await capture(resultPath, "flightDeckRunning", {
    missions: running.length,
    states: running.map((snapshot) => snapshot.mission.state),
    sessions: sessions.length,
  });

  currentStage = "sealing-flight";
  const final = await within(
    journey.continueReadyMissions(preferred),
    180_000,
    "final Mission Flight Deck continuation",
  );
  if (final.missions !== 2 || final.verified !== 2 || final.sessionIds.length !== 0 || final.remainingReady !== 0) {
    throw new Error(
      `the final flight advanced ${final.missions}, verified ${final.verified}, started ${final.sessionIds.length}, and left ${final.remainingReady}`,
    );
  }
  const integrating = await Promise.all(reviewed.map((snapshot) => journey.mission(snapshot.mission.mission_id)));
  for (const snapshot of integrating) {
    assertMission(snapshot, "integrating", ["passed"]);
    if (snapshot.tasks[0]?.artifact_paths.length !== 2) {
      throw new Error(`${snapshot.mission.name} did not seal both reviewed Artifacts`);
    }
  }
  await showMissions();
  await capture(resultPath, "flightDeckIntegrating", {
    missions: integrating.length,
    states: integrating.map((snapshot) => snapshot.mission.state),
    passed: integrating.map((snapshot) => snapshot.mission.passed_tasks),
  });

  currentStage = "reviewing-landing";
  const firstLanding = integrating[0];
  const firstTask = firstLanding.tasks[0];
  if (!firstTask?.workspace) throw new Error(`${firstLanding.mission.name} has no sealed Task workspace`);
  const firstSource = path.join(firstTask.workspace, ...firstTask.artifact_paths[0].split("/"));
  const firstSourceBeforeReview = await readFile(firstSource);
  await writeFile(firstSource, "changed after Receipt sealing and before review\n", "utf8");
  await assert.rejects(
    journey.reviewMissionLanding(firstLanding.mission.mission_id),
    /Receipt Artifact evidence mismatch/,
  );
  await writeFile(firstSource, firstSourceBeforeReview);
  await journey.reviewMissionLanding(firstLanding.mission.mission_id);
  await waitFor(
    () => vscode.window.tabGroups.all.some((group) =>
      group.tabs.some((tab) => tab.label.includes("Receipt Landing"))
    ),
    10_000,
    "the native Receipt Landing multi-diff",
  );
  await delay(1_200);
  await capture(resultPath, "missionLandingReview", {
    mission: firstLanding.mission.name,
    artifacts: firstLanding.tasks[0]?.artifact_paths.length,
    editors: vscode.window.tabGroups.all.flatMap((group) => group.tabs).length,
  });

  currentStage = "completing-landing";
  const firstTarget = path.join(firstLanding.mission.project, ...firstLanding.tasks[0].artifact_paths[0].split("/"));
  const firstTargetBeforeReview = await readFile(firstTarget);
  await writeFile(firstTarget, "changed after Landing review\n", "utf8");
  await assert.rejects(
    journey.applyMissionLanding(firstLanding.mission.mission_id),
    /Project Artifact changed/,
  );
  assertMission(await journey.mission(firstLanding.mission.mission_id), "integrating", ["passed"]);
  await writeFile(firstTarget, firstTargetBeforeReview);

  const dirtyDocument = await vscode.workspace.openTextDocument(firstTarget);
  const dirtyEditor = await vscode.window.showTextDocument(dirtyDocument, { preview: false });
  await dirtyEditor.edit((edit) => edit.insert(new vscode.Position(0, 0), "unsaved "));
  await assert.rejects(
    journey.applyMissionLanding(firstLanding.mission.mission_id),
    /Unsaved editor/,
  );
  await vscode.commands.executeCommand("workbench.action.files.revert");
  if (dirtyDocument.isDirty) throw new Error("the dirty editor fixture did not revert");
  await vscode.commands.executeCommand("workbench.action.closeActiveEditor");

  const targetDirectory = path.dirname(firstTarget);
  const originalDirectory = `${targetDirectory}-real`;
  await rename(targetDirectory, originalDirectory);
  try {
    await symlink(originalDirectory, targetDirectory, process.platform === "win32" ? "junction" : "dir");
    await assert.rejects(
      journey.applyMissionLanding(firstLanding.mission.mission_id),
      /Symbolic link/,
    );
  } finally {
    if (process.platform === "win32") await rmdir(targetDirectory).catch(() => undefined);
    else await unlink(targetDirectory).catch(() => undefined);
    await rename(originalDirectory, targetDirectory);
  }

  currentStage = "confirming-landing";
  const confirmation = Promise.resolve(vscode.commands.executeCommand(
    "runtrol.reviewMissionLanding",
    { mission: firstLanding.mission },
  ));
  await delay(1_200);
  await capture(resultPath, "missionLandingApplyConfirmation", {
    mission: firstLanding.mission.name,
    artifacts: firstLanding.tasks[0]?.artifact_paths.length,
  });
  await vscode.commands.executeCommand("notifications.focusToasts");
  await vscode.commands.executeCommand("notification.acceptPrimaryAction");
  await waitFor(
    async () => (await journey.mission(firstLanding.mission.mission_id)).mission.state === "completed",
    60_000,
    "the public Landing action to complete the first Mission",
  );
  await settleNotificationCommand(confirmation, 10_000);
  const completed = await journey.mission(firstLanding.mission.mission_id);
  assertMission(completed, "completed", ["passed"]);
  await assertAppliedArtifacts(firstLanding);
  const secondStillWaiting = await journey.mission(integrating[1].mission.mission_id);
  assertMission(secondStillWaiting, "integrating", ["passed"]);
  await showMissions();
  await capture(resultPath, "missionLandingCompleted", {
    completed: completed.mission.name,
    remaining: secondStillWaiting.mission.name,
  });

  currentStage = "reviewing-next-landing";
  await journey.reviewMissionLanding(secondStillWaiting.mission.mission_id);
  await waitFor(
    () => vscode.window.tabGroups.all.some((group) =>
      group.tabs.some((tab) => tab.label.includes("Receipt Landing"))
    ),
    10_000,
    "the next native Receipt Landing multi-diff",
  );
  await delay(1_200);
  await capture(resultPath, "missionLandingNext", {
    mission: secondStillWaiting.mission.name,
    completed: completed.mission.name,
  });
  const secondTask = secondStillWaiting.tasks[0];
  if (!secondTask.workspace) throw new Error(`${secondStillWaiting.mission.name} has no sealed Task workspace`);
  const secondSource = path.join(secondTask.workspace, ...secondTask.artifact_paths[0].split("/"));
  const secondSourceAtReview = await readFile(secondSource);
  await writeFile(secondSource, "changed Receipt Artifact after review\n", "utf8");
  await assert.rejects(
    journey.applyMissionLanding(secondStillWaiting.mission.mission_id),
    /Receipt Artifact evidence mismatch/,
  );
  assertMission(await journey.mission(secondStillWaiting.mission.mission_id), "integrating", ["passed"]);
  await writeFile(secondSource, secondSourceAtReview);
  await assert.rejects(
    journey.applyMissionLanding(secondStillWaiting.mission.mission_id),
    /integrated tree does not match passing Task evidence/,
  );
  assertMission(await journey.mission(secondStillWaiting.mission.mission_id), "integrating", ["passed"]);
  const secondTarget = path.join(secondStillWaiting.mission.project, ...secondTask.artifact_paths[0].split("/"));
  assert.match(await readFile(secondTarget, "utf8"), /changed by Gate/);
  await writeFile(secondTarget, await readFile(secondSource));
  const allCompleted = await journey.applyMissionLanding(secondStillWaiting.mission.mission_id);
  assertMission(allCompleted, "completed", ["passed"]);
  await assertAppliedArtifacts(secondStillWaiting);
  await unlink(path.join(secondStillWaiting.mission.project, ".gate-mutated-once"));

  currentStage = "closing";
  for (const session of [...sessions]) {
    await journey.close(session, true).catch(() => undefined);
    sessions.splice(sessions.indexOf(session), 1);
  }
  await writeFile(
    resultPath,
    JSON.stringify({
      stage: "complete",
      missions: 2,
      started: 2,
      verified: 2,
      landed: 2,
      driftBlocks: 5,
      gateMutationBlocks: 1,
    }),
    "utf8",
  );
}

async function prepareProject(folder: string, name: string, id: string, provider: string): Promise<void> {
  await mkdir(path.join(folder, "instructions"), { recursive: true });
  await mkdir(path.join(folder, "outputs"), { recursive: true });
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  const instructionPath = path.join(folder, "instructions", "task.md");
  await writeFile(instructionPath, "Reply with exactly: done\n", "utf8");
  await writeFile(path.join(folder, "outputs", "result.txt"), "base\n", "utf8");
  await writeFile(
    path.join(folder, "gate.cjs"),
    "const fs = require('node:fs');\n"
      + "const path = require('node:path');\n"
      + "const cwd = process.cwd();\n"
      + "if (!cwd.includes('.runtrol-worktrees')) {\n"
      + "  const marker = path.join(cwd, '.gate-mutated-once');\n"
      + "  if (!fs.existsSync(marker)) {\n"
      + "    fs.writeFileSync(marker, '1');\n"
      + "    fs.appendFileSync(path.join(cwd, 'outputs', 'result.txt'), 'changed by Gate\\n');\n"
      + "  }\n"
      + "}\n",
    "utf8",
  );
  const instructionSha256 = await digest(instructionPath);
  const mission = `schema = "runtrol.dev/mission/v1alpha1"
name = ${JSON.stringify(name)}
project_id = ${JSON.stringify(id)}
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
id = "implement"
instruction_ref = "instructions/task.md"
instruction_sha256 = "${instructionSha256}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs"]
gate_refs = [${JSON.stringify(id === "flight-beta" ? "flight-deck-eye-mutates-once" : "flight-deck-eye-check")}]
`;
  await writeFile(path.join(folder, "mission.toml"), mission, "utf8");
  await git(folder, "add", "--", "instructions", "outputs", "gate.cjs", "mission.toml");
  await git(folder, "commit", "-m", "flight deck fixture");
}

async function writeTaskArtifact(snapshot: MissionSnapshot, content: string): Promise<void> {
  const task = snapshot.tasks[0];
  if (!task?.workspace || task.output_roots.length !== 1) {
    throw new Error(`${snapshot.mission.name} has no exact prepared Artifact path`);
  }
  await writeFile(path.join(task.workspace, "outputs", "result.txt"), content, "utf8");
  await writeFile(path.join(task.workspace, "outputs", "summary.txt"), `${snapshot.mission.name} summary\n`, "utf8");
}

async function assertAppliedArtifacts(snapshot: MissionSnapshot): Promise<void> {
  const task = snapshot.tasks[0];
  if (!task?.workspace) throw new Error(`${snapshot.mission.name} has no sealed Task workspace`);
  for (const artifact of task.artifact_paths) {
    const target = path.join(snapshot.mission.project, ...artifact.split("/"));
    assert.deepEqual(
      await readFile(target),
      await readFile(path.join(task.workspace, ...artifact.split("/"))),
      `${snapshot.mission.name} ${artifact} did not land as exact Receipt bytes`,
    );
  }
}

function assertMission(snapshot: MissionSnapshot, missionState: string, taskStates: readonly string[]): void {
  if (snapshot.mission.state !== missionState) {
    throw new Error(`${snapshot.mission.name} reached ${snapshot.mission.state}, not ${missionState}`);
  }
  const actual = snapshot.tasks.map((task) => task.state);
  if (actual.length !== taskStates.length || actual.some((state, index) => state !== taskStates[index])) {
    throw new Error(`${snapshot.mission.name} Task states are ${actual.join(", ")}, not ${taskStates.join(", ")}`);
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

async function waitFor(condition: () => boolean | Promise<boolean>, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!await condition()) {
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

async function settleNotificationCommand(work: PromiseLike<unknown>, deadlineMs: number): Promise<void> {
  let settled = false;
  let failure: unknown;
  void Promise.resolve(work).then(
    () => {
      settled = true;
    },
    (error) => {
      failure = error;
      settled = true;
    },
  );
  const deadline = Date.now() + deadlineMs;
  while (!settled) {
    await vscode.commands.executeCommand("notifications.clearAll");
    if (Date.now() > deadline) throw new Error("the public Landing command did not settle after completion");
    await delay(100);
  }
  if (failure) throw failure;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
