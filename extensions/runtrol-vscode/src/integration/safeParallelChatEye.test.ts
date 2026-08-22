import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, readFile, realpath, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import * as vscode from "vscode";

import type { IsolatedWorkspaceLine } from "../protocol";
import type { ProviderLine, SessionLine } from "../runtimeTypes";
import { extensionUnderTest } from "./extensionUnderTest.test";

type IsolationEvidence = {
  workspaces: readonly IsolatedWorkspaceLine[];
  roots: readonly string[];
};

type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  openDraft(workspace: string | null, providerId?: string): Promise<void>;
  alsoAsk(providerId: string): Promise<void>;
  sendFocusedDraft(text: string): Promise<string>;
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
  reconnect(): Promise<void>;
  close(session: string, now?: boolean): Promise<void>;
  isolationEvidence(): Promise<IsolationEvidence>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: JourneyApi;
};

const executeFile = promisify(execFile);
const PROMPT = "Reply with exactly: isolated parallel complete";
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
  const home = requiredEnvironment("RUNTROL_HOME");
  await prepareProject(folder);
  const baseCommit = await git(folder, "rev-parse", "HEAD");
  const baseCommon = await commonGitDirectory(folder);

  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  while (!extension.isActive) await delay(25);
  await within(extension.exports.ready, 60_000, "extension initialization");
  const journey = extension.exports.journey;
  if (!journey) throw new Error("the journey API is unavailable");

  currentStage = "waiting-for-two-providers";
  await waitFor(
    () => journey.providers().filter((provider) => provider.installation.state === "usable").length >= 2,
    120_000,
    "two installed usable providers",
  );
  const primary = journey.providers().find((provider) => (
    provider.providerId === preferred && provider.installation.state === "usable"
  ));
  if (!primary) throw new Error(`the preferred provider ${preferred} is not usable`);
  const secondary = journey.providers().find((provider) => (
    provider.providerId !== primary.providerId && provider.installation.state === "usable"
  ));
  if (!secondary) throw new Error("a second usable provider is required for safe parallel chat evidence");

  const before = new Set(journey.sessions().map((session) => session.sessionId));
  await journey.openDraft(folder, primary.providerId);
  await journey.alsoAsk(secondary.providerId);
  await delay(700);
  await capture(resultPath, "isolatedDraft", {
    primary: primary.providerId,
    secondary: secondary.providerId,
  });

  currentStage = "starting-isolated-parallel-chat";
  const first = await within(journey.sendFocusedDraft(PROMPT), 300_000, "the isolated parallel send");
  await waitFor(
    () => journey.sessions().filter((session) => !before.has(session.sessionId)).length === 2,
    60_000,
    "two new isolated Runtime sessions",
  );
  const opened = journey.sessions().filter((session) => !before.has(session.sessionId));
  sessions.push(...opened.map((session) => session.sessionId));
  assert.equal(opened.length, 2);
  assert.ok(opened.some((session) => session.sessionId === first));
  assert.deepEqual(
    new Set(opened.map((session) => session.providerId)),
    new Set([primary.providerId, secondary.providerId]),
  );

  const generatedRoot = await realpath(path.join(path.dirname(folder), ".runtrol-worktrees"));
  const exactWorkspaces = await Promise.all(opened.map(async (session) => realpath(session.workspace)));
  assert.equal(new Set(exactWorkspaces.map(pathIdentity)).size, 2, "each provider must have a different worktree");
  for (const workspace of exactWorkspaces) {
    assert.notEqual(pathIdentity(workspace), pathIdentity(await realpath(folder)));
    assert.ok(isInside(generatedRoot, workspace), `${workspace} is outside ${generatedRoot}`);
    assert.equal(await commonGitDirectory(workspace), baseCommon);
    assert.equal(await git(workspace, "rev-parse", "HEAD"), baseCommit);
  }
  assert.equal(await git(folder, "status", "--porcelain"), "", "the selected checkout changed during fan-out");

  const beforeRestart = await journey.isolationEvidence();
  assertBoundOwnership(beforeRestart, opened);
  assertExactRoots(beforeRestart.roots, exactWorkspaces, true);
  const registry = await readFile(path.join(home, "isolated-workspaces.json"), "utf8");
  assert.equal(registry.includes(PROMPT), false, "the Core registry must not store conversation text");

  await waitFor(
    () => sessions.every((session) => {
      const lifecycle = journey.sessions().find((candidate) => candidate.sessionId === session)?.lifecycle;
      return lifecycle === "hotRunning" || lifecycle === "hotIdle";
    }),
    30_000,
    "both provider processes to accept the submitted input",
  );
  await delay(4_000);
  const settledBeforeRestart = journey.sessions().filter((session) => (
    sessions.includes(session.sessionId) && session.lifecycle === "hotIdle"
  )).length;
  await showChats();
  await capture(resultPath, "isolatedGrid", {
    sessions: opened.length,
    providers: opened.map((session) => session.providerId),
    distinctWorkspaces: new Set(exactWorkspaces.map(pathIdentity)).size,
    baseUnchanged: true,
    settledBeforeRestart,
  });

  currentStage = "fault:restartCore";
  await writeFile(resultPath, JSON.stringify({ stage: currentStage, sessions }), "utf8");
  await waitForFile(`${resultPath}.restarted`, 30_000, "the isolated Core restart");
  await within(journey.reconnect(), 300_000, "Studio reconnect after Core restart");
  await waitFor(
    () => sessions.every((session) => journey.sessions().some((candidate) => candidate.sessionId === session)),
    60_000,
    "both isolated sessions after reconnect",
  );
  const recovered = journey.sessions().filter((session) => sessions.includes(session.sessionId));
  assert.deepEqual(
    recovered.map((session) => pathIdentity(session.workspace)).sort(),
    exactWorkspaces.map(pathIdentity).sort(),
  );
  const afterRestart = await journey.isolationEvidence();
  assertBoundOwnership(afterRestart, recovered);
  assertExactRoots(afterRestart.roots, exactWorkspaces, true);
  await showChats();
  await capture(resultPath, "isolatedRecovered", {
    sessions: recovered.length,
    ownership: afterRestart.workspaces.length,
    sameWorkspaces: true,
  });

  currentStage = "closing";
  for (const session of [...sessions]) {
    await within(journey.close(session, true), 90_000, `closing isolated session ${session}`);
    sessions.splice(sessions.indexOf(session), 1);
  }
  const afterClose = await journey.isolationEvidence();
  assert.equal(afterClose.workspaces.length, 0, "closed isolated workspaces remain owned");
  assertExactRoots(afterClose.roots, exactWorkspaces, false);
  for (const workspace of exactWorkspaces) {
    await assert.rejects(access(workspace), /ENOENT/);
  }
  assert.equal(await git(folder, "status", "--porcelain"), "", "the selected checkout changed after cleanup");

  await writeFile(
    resultPath,
    JSON.stringify({
      stage: "complete",
      providers: [primary.providerId, secondary.providerId],
      sessions: opened.length,
      distinctWorkspaces: true,
      exactBaseCommit: baseCommit,
      baseUnchanged: true,
      restartRecovered: true,
      settledBeforeRestart,
      cleanupRemoved: true,
      rootsRevoked: true,
      registryStoredConversation: false,
    }),
    "utf8",
  );
}

function assertBoundOwnership(evidence: IsolationEvidence, sessions: readonly SessionLine[]): void {
  assert.equal(evidence.workspaces.length, sessions.length);
  for (const session of sessions) {
    const owned = evidence.workspaces.find((workspace) => workspace.session_id === session.sessionId);
    assert.ok(owned, `session ${session.sessionId} has no durable worktree ownership`);
    assert.equal(owned.state, "bound");
    assert.equal(pathIdentity(owned.workspace), pathIdentity(session.workspace));
  }
}

function assertExactRoots(roots: readonly string[], workspaces: readonly string[], present: boolean): void {
  for (const workspace of workspaces) {
    assert.equal(
      roots.some((root) => pathIdentity(root) === pathIdentity(workspace)),
      present,
      `${workspace} exact Runtime root presence must be ${present}`,
    );
  }
}

async function prepareProject(folder: string): Promise<void> {
  await git(folder, "init", "--initial-branch=main");
  await git(folder, "config", "user.email", "fixture@runtrol.invalid");
  await git(folder, "config", "user.name", "Runtrol Fixture");
  await writeFile(path.join(folder, "README.md"), "# Safe parallel chat fixture\n", "utf8");
  await git(folder, "add", "--", "README.md");
  await git(folder, "commit", "-m", "base fixture");
}

async function showChats(): Promise<void> {
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  await vscode.commands.executeCommand("runtrol.sessions.focus");
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  await delay(1_500);
}

async function commonGitDirectory(folder: string): Promise<string> {
  const common = await git(folder, "rev-parse", "--git-common-dir");
  return pathIdentity(await realpath(path.resolve(folder, common)));
}

async function git(folder: string, ...arguments_: string[]): Promise<string> {
  const result = await executeFile("git", arguments_, { cwd: folder, windowsHide: true });
  return result.stdout.trim();
}

function isInside(parent: string, child: string): boolean {
  const relative = path.relative(parent, child);
  return relative.length > 0 && !relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative);
}

function pathIdentity(value: string): string {
  const resolved = path.resolve(value);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeFile(resultPath, JSON.stringify({ stage: currentStage, ...facts }), "utf8");
  await waitForFile(`${resultPath}.captured.${pose}`, 60_000, `the ${pose} capture`);
}

async function waitFor(condition: () => boolean, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await delay(100);
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

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
