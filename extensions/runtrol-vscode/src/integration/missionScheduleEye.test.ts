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
  scheduleMission(
    missionId: string,
    dueUnixMs: number,
    operatorChoiceProvider: string | null,
  ): Promise<MissionSnapshot>;
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
    const phase = process.env.RUNTROL_MISSION_SCHEDULE_PHASE;
    if (phase === "schedule") {
      const scheduled = await schedulePending(resultPath);
      await writeFile(resultPath, JSON.stringify({
        stage: "complete",
        phase: "scheduled",
        missionId: scheduled.missionId,
        dueUnixMs: scheduled.dueUnixMs,
        provider: scheduled.provider,
        scheduleId: scheduled.scheduleId,
      }), "utf8");
    } else if (phase === "observe") {
      const provider = requiredEnvironment("RUNTROL_EYE_PROVIDER");
      const journey = await readyJourney(provider);
      await observeStarted(
        resultPath,
        journey,
        requiredEnvironment("RUNTROL_MISSION_SCHEDULE_ID"),
        Number(requiredEnvironment("RUNTROL_MISSION_SCHEDULE_DUE")),
        provider,
        sessions,
        Number(requiredEnvironment("RUNTROL_MISSION_STUDIO_CLOSED")),
      );
    } else {
      await eyePass(resultPath, sessions);
    }
  } catch (error) {
    await writeFile(resultPath, JSON.stringify({
      stage: currentStage,
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    }), "utf8");
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
  const scheduled = await schedulePending(resultPath);
  await observeStarted(
    resultPath,
    scheduled.journey,
    scheduled.missionId,
    scheduled.dueUnixMs,
    scheduled.provider,
    sessions,
    0,
  );
}

async function schedulePending(resultPath: string): Promise<{
  journey: JourneyApi;
  missionId: string;
  dueUnixMs: number;
  provider: string;
  scheduleId: string;
}> {
  const folder = requiredEnvironment("RUNTROL_EYE_FOLDER");
  const provider = requiredEnvironment("RUNTROL_EYE_PROVIDER");
  await prepareProject(folder, provider);
  const journey = await readyJourney(provider);

  await journey.registerMissionGate("schedule-eye-check", "git", ["diff", "--check", "HEAD"]);
  const reviewed = await journey.validateMissionFile(path.join(folder, "scheduled.toml"));
  const dueUnixMs = Date.now() + 15_000;
  const scheduled = await journey.scheduleMission(reviewed.mission.mission_id, dueUnixMs, provider);
  assert.equal(scheduled.mission.state, "validated");
  assert.equal(scheduled.mission.schedule?.state, "pending");
  assert.equal(scheduled.mission.schedule?.due_unix_ms, dueUnixMs);
  assert.equal(scheduled.mission.schedule?.providers[0]?.provider_runtime_id, provider);
  const scheduleId = scheduled.mission.schedule?.schedule_id;
  if (!scheduleId) throw new Error("the pending Mission has no schedule identity");
  await showMissions(journey);
  await capture(resultPath, "schedulePending", {
    mission: scheduled.mission.mission_id,
    dueUnixMs,
    provider,
    scheduleId,
  });

  return {
    journey,
    missionId: scheduled.mission.mission_id,
    dueUnixMs,
    provider,
    scheduleId,
  };
}

async function observeStarted(
  resultPath: string,
  journey: JourneyApi,
  missionId: string,
  dueUnixMs: number,
  provider: string,
  sessions: string[],
  studioClosedUnixMs: number,
): Promise<void> {
  currentStage = "waiting-for-core-owned-wake";
  await delay(Math.max(0, dueUnixMs - Date.now()) + 6_000);
  const launched = await waitForMission(
    journey,
    missionId,
    (snapshot) => snapshot.mission.state === "running"
      && snapshot.mission.schedule?.state === "started"
      && snapshot.tasks[0]?.state === "running"
      && snapshot.tasks[0]?.session_id !== null,
    60_000,
  );
  const sessionId = launched.tasks[0]?.session_id;
  if (!sessionId) throw new Error("the due schedule started no managed session");
  sessions.push(sessionId);
  assert.ok(journey.sessions().some((session) => session.sessionId === sessionId));
  await showMissions(journey);
  await capture(resultPath, "scheduleStarted", {
    mission: launched.mission.mission_id,
    scheduleState: launched.mission.schedule?.state,
    taskState: launched.tasks[0]?.state,
    sessionId,
    dueUnixMs,
    observedUnixMs: Date.now(),
    studioClosedUnixMs,
  });

  currentStage = "closing";
  await journey.close(sessionId, true);
  sessions.splice(sessions.indexOf(sessionId), 1);
  await writeFile(resultPath, JSON.stringify({
    stage: "complete",
    scheduled: true,
    coreOwnedStart: true,
    studioClosedBeforeDue: studioClosedUnixMs > 0 && studioClosedUnixMs < dueUnixMs,
    studioClosedUnixMs,
    dueUnixMs,
    sessionId,
    provider,
  }), "utf8");
}

async function readyJourney(provider: string): Promise<JourneyApi> {
  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  while (!extension.isActive) await delay(25);
  await within(extension.exports.ready, 60_000, "extension initialization");
  const journey = extension.exports.journey;
  if (!journey) throw new Error("the journey API is unavailable");
  await waitForProvider(journey, provider, 90_000);
  return journey;
}

async function prepareProject(folder: string, provider: string): Promise<void> {
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  await mkdir(path.join(folder, "instructions"), { recursive: true });
  await mkdir(path.join(folder, "outputs"), { recursive: true });
  const instruction = path.join(folder, "instructions", "scheduled.md");
  await writeFile(instruction, "Reply with exactly: scheduled wake complete\n", "utf8");
  await writeFile(path.join(folder, "outputs", "result.txt"), "reviewed Artifact\n", "utf8");
  await git(folder, "add", "--", "instructions", "outputs");
  await git(folder, "commit", "-m", "base fixture");
  const instructionSha256 = createHash("sha256").update(await readFile(instruction)).digest("hex");
  const mission = `schema = "runtrol.dev/mission/v1alpha1"
name = "Core-owned scheduled Mission"
project_id = "mission-schedule-eye"
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
id = "scheduled"
instruction_ref = "instructions/scheduled.md"
instruction_sha256 = "${instructionSha256}"
workspace_mode = "isolated_worktree"
provider_selector = ${JSON.stringify(`runtime:${provider}`)}
output_roots = ["outputs/result.txt"]
gate_refs = ["schedule-eye-check"]
`;
  await writeFile(path.join(folder, "scheduled.toml"), mission, "utf8");
  await git(folder, "add", "--", "scheduled.toml");
  await git(folder, "commit", "-m", "schedule fixture");
}

async function showMissions(journey: JourneyApi): Promise<void> {
  await journey.refreshMissions();
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
      throw new Error(`timed out with Mission ${snapshot.mission.state}, schedule ${snapshot.mission.schedule?.state}, and Task ${snapshot.tasks[0]?.state}`);
    }
    await delay(250);
  }
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
