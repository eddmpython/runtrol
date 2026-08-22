import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import * as vscode from "vscode";

import type { MissionSnapshot } from "../protocol";
import type { ProviderLine, SessionLine } from "../runtimeTypes";
import { extensionUnderTest } from "./extensionUnderTest.test";

type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  registerMissionGate(gateId: string, program: string, arguments_: string[]): Promise<void>;
  validateMissionFile(file: string): Promise<MissionSnapshot>;
  continueMission(
    missionId: string,
    operatorChoiceProvider: string,
  ): Promise<{ snapshot: MissionSnapshot; sessionIds: readonly string[]; verified: number }>;
  reconnect(): Promise<void>;
  refreshMissions(): Promise<void>;
  mission(missionId: string): Promise<MissionSnapshot>;
  close(session: string, now?: boolean): Promise<void>;
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
  const provider = requiredEnvironment("RUNTROL_EYE_PROVIDER");
  await prepareProject(folder, provider);

  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  while (!extension.isActive) await delay(25);
  await within(extension.exports.ready, 60_000, "extension initialization");
  const journey = extension.exports.journey;
  if (!journey) throw new Error("the journey API is unavailable");
  await waitForProvider(journey, provider, 90_000);

  await journey.registerMissionGate("recovery-eye-check", "git", ["diff", "--check", "HEAD"]);
  const reviewed = await journey.validateMissionFile(path.join(folder, "recovery.toml"));
  const started = await within(
    journey.continueMission(reviewed.mission.mission_id, provider),
    300_000,
    "the first exact Mission wave",
  );
  const originalSession = started.sessionIds[0];
  if (!originalSession) throw new Error("the interrupted Mission started no Runtime session");
  sessions.push(originalSession);
  assertMission(started.snapshot, "running", "running");
  await showMissions();
  await capture(resultPath, "recoveryInFlight", {
    mission: started.snapshot.mission.mission_id,
    session: originalSession,
  });

  currentStage = "fault:restartCore";
  await writeFile(resultPath, JSON.stringify({ stage: currentStage, session: originalSession }), "utf8");
  await waitForFile(`${resultPath}.restarted`, 30_000, "the isolated Core restart");
  sessions.splice(sessions.indexOf(originalSession), 1);
  await within(journey.reconnect(), 300_000, "Studio reconnect after Core restart");
  await waitForProvider(journey, provider, 90_000);

  const blocked = await waitForMission(
    journey,
    reviewed.mission.mission_id,
    (snapshot) => snapshot.mission.state === "blocked" && snapshot.tasks[0]?.state === "blocked",
    30_000,
  );
  assert.equal(blocked.tasks[0]?.session_id, null);
  await showMissions();
  await capture(resultPath, "recoveryBlocked", {
    state: blocked.mission.state,
    task: blocked.tasks[0]?.state,
    oldSessionGone: !journey.sessions().some((session) => session.sessionId === originalSession),
  });

  currentStage = "cancelling-recovery";
  const cancelling = Promise.resolve(vscode.commands.executeCommand(
    "runtrol.recoverInterruptedMission",
    { mission: blocked.mission },
  ));
  await delay(1_200);
  await capture(resultPath, "recoveryCancellation", {
    missionSha256: blocked.mission_sha256,
    task: blocked.tasks[0]?.key,
  });
  await within(cancelling, 30_000, "cancelled interrupted Mission recovery");
  const unchanged = await journey.mission(blocked.mission.mission_id);
  assert.equal(unchanged.mission.state, "blocked");
  assert.equal(unchanged.tasks[0]?.state, "blocked");
  assert.equal(unchanged.mission_sha256, blocked.mission_sha256);

  currentStage = "confirming-recovery";
  const recovering = Promise.resolve(vscode.commands.executeCommand(
    "runtrol.recoverInterruptedMission",
    { mission: blocked.mission },
  ));
  await delay(1_200);
  await capture(resultPath, "recoveryConfirmation", {
    missionSha256: blocked.mission_sha256,
    task: blocked.tasks[0]?.key,
    provider,
  });
  await within(recovering, 300_000, "the public interrupted Mission recovery");

  const recovered = await waitForMission(
    journey,
    reviewed.mission.mission_id,
    (snapshot) => snapshot.mission.state === "running"
      && snapshot.tasks[0]?.state === "running"
      && snapshot.tasks[0]?.session_id !== null,
    30_000,
  );
  const freshSession = recovered.tasks[0]?.session_id;
  if (!freshSession) throw new Error("recovery started no fresh Runtime session");
  assert.notEqual(freshSession, originalSession);
  sessions.push(freshSession);
  await showMissions();
  await capture(resultPath, "recoveryRunning", {
    state: recovered.mission.state,
    task: recovered.tasks[0]?.state,
    oldSession: originalSession,
    freshSession,
  });

  currentStage = "closing";
  for (const session of [...sessions]) {
    await journey.close(session, true).catch(() => undefined);
    sessions.splice(sessions.indexOf(session), 1);
  }
  await writeFile(
    resultPath,
    JSON.stringify({
      stage: "complete",
      blocked: true,
      cancelledUnchanged: true,
      recovered: true,
      oldSession: originalSession,
      freshSession,
      distinctSession: freshSession !== originalSession,
    }),
    "utf8",
  );
}

async function prepareProject(folder: string, provider: string): Promise<void> {
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  await mkdir(path.join(folder, "instructions"), { recursive: true });
  await mkdir(path.join(folder, "outputs"), { recursive: true });
  const instruction = path.join(folder, "instructions", "recover.md");
  await writeFile(instruction, "Reply with exactly: recovered\n", "utf8");
  await writeFile(path.join(folder, "outputs", "result.txt"), "reviewed Artifact\n", "utf8");
  await git(folder, "add", "--", "instructions", "outputs");
  await git(folder, "commit", "-m", "base fixture");

  const instructionSha256 = createHash("sha256").update(await readFile(instruction)).digest("hex");
  const mission = `schema = "runtrol.dev/mission/v1alpha1"
name = "interrupted Mission recovery"
project_id = "mission-recovery-eye"
base_ref = "main"
require_clean_base = true
completion_policy = "all_tasks"

[limits]
max_parallel_tasks = 1
max_hot_providers = 1
max_runs_per_task = 2
max_repair_cycles = 0
stop_on_critical_failure = true

[[tasks]]
id = "recover"
instruction_ref = "instructions/recover.md"
instruction_sha256 = "${instructionSha256}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs/result.txt"]
gate_refs = ["recovery-eye-check"]
`;
  await writeFile(path.join(folder, "recovery.toml"), mission, "utf8");
  await git(folder, "add", "--", "recovery.toml");
  await git(folder, "commit", "-m", "recovery fixture");
}

async function showMissions(): Promise<void> {
  const extension = extensionUnderTest<ExtensionApi>();
  await extension.exports.journey?.refreshMissions();
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  await vscode.commands.executeCommand("runtrol.missions.focus");
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  await delay(1_200);
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeFile(resultPath, JSON.stringify({ stage: currentStage, ...facts }), "utf8");
  await waitForFile(`${resultPath}.captured.${pose}`, 60_000, `the ${pose} capture`);
}

async function waitForProvider(journey: JourneyApi, provider: string, deadlineMs: number): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!journey.providers().some((candidate) => (
    candidate.providerId === provider && candidate.installation.state === "usable"
  ))) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for installed provider ${provider}`);
    await delay(100);
  }
}

async function waitForMission(
  journey: JourneyApi,
  missionId: string,
  condition: (snapshot: MissionSnapshot) => boolean,
  deadlineMs: number,
): Promise<MissionSnapshot> {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    const snapshot = await journey.mission(missionId);
    if (condition(snapshot)) return snapshot;
    if (Date.now() > deadline) {
      throw new Error(`timed out with Mission ${snapshot.mission.state} and Task ${snapshot.tasks[0]?.state}`);
    }
    await delay(250);
  }
}

function assertMission(snapshot: MissionSnapshot, missionState: string, taskState: string): void {
  assert.equal(snapshot.mission.state, missionState);
  assert.equal(snapshot.tasks[0]?.state, taskState);
}

async function waitForFile(file: string, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    try {
      await readFile(file, "utf8");
      return;
    } catch {
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
      await delay(250);
    }
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

async function git(folder: string, ...arguments_: string[]): Promise<void> {
  await executeFile("git", arguments_, { cwd: folder, windowsHide: true });
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
