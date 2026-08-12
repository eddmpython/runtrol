import { readFile, writeFile } from "node:fs/promises";
import { performance } from "node:perf_hooks";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";

let currentStage = "starting";

type ExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<{
    baselineFrameP95Ms: number;
    frameP95Ms: number;
    frameOverrunP95Ms: number;
    inputP95Ms: number;
    scrollP95Ms: number;
    maxPendingFrames: number;
    producedFrames: number;
    droppedFrames: number;
    visibleCharacters: number;
    visibleItems: number;
  }>;
  measureSessionManagement?(sessionIds: readonly string[]): Promise<{
    sessionCount: number;
    hotSessionCount: number;
    coldResumeMs: number;
    sessionSwitchP95Ms: number;
    resumedFrom: string;
    resumedTo: string;
    restoreSession: string;
    restoreWorkspace: string;
  }>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
};

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    if (process.env.RUNTROL_VSCODE_PHASE === "restore") {
      const restored = await measureRestore(requiredEnvironment("RUNTROL_VSCODE_RESTORE_SESSION"));
      await writeFile(resultPath, JSON.stringify(restored), "utf8");
    } else {
      const measured = await measure(resultPath);
      await writeFile(resultPath, JSON.stringify(measured), "utf8");
    }
  } catch (error) {
    let progress: Record<string, unknown> = {};
    try {
      const recorded: unknown = JSON.parse(await readFile(resultPath, "utf8"));
      if (recorded && typeof recorded === "object" && !Array.isArray(recorded)) {
        progress = recorded as Record<string, unknown>;
      }
    } catch {
      // A failure before the first checkpoint has no progress record to preserve.
    }
    await writeFile(
      resultPath,
      JSON.stringify({
        ...progress,
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        stage: currentStage,
      }),
      "utf8",
    );
    throw error;
  }
}

async function measure(resultPath: string): Promise<Record<string, number | string>> {
  const core = requiredEnvironment("RUNTROL_TEST_CORE");
  const configuredCore = vscode.workspace.getConfiguration("runtrol").get<string>("corePath");
  if (configuredCore !== core) {
    throw new Error(`the isolated VS Code profile configured unexpected Core ${String(configuredCore)}`);
  }
  const extension = extensionUnderTest<ExtensionApi>();
  const rssBefore = process.memoryUsage().rss;
  const activationStarted = performance.now();
  const api = await within(extension.activate() as Promise<ExtensionApi>, 5_000, "extension activation");
  await checkpoint(resultPath, "activated");
  await within(api.ready, 5_000, "extension initialization");
  const activationMs = performance.now() - activationStarted;
  await checkpoint(resultPath, "ready");

  await checkpoint(resultPath, "view-opening");
  const viewStarted = performance.now();
  try {
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      5_000,
      "opening the Runtrol view",
    );
  } catch (error) {
    throw new Error(`Runtrol view command failed: ${error instanceof Error ? error.message : String(error)}`, {
      cause: error,
    });
  }
  await within(
    vscode.commands.executeCommand("runtrol.conversation.focus"),
    5_000,
    "focusing the Runtrol Webview",
  );
  const openViewMs = performance.now() - viewStarted;
  await checkpoint(resultPath, "view-open");

  for (let index = 0; index < 5; index += 1) {
    await within(api.refresh(), 5_000, "refresh warmup");
  }
  await checkpoint(resultPath, "warmed");
  const refreshSamples: number[] = [];
  for (let index = 0; index < 40; index += 1) {
    const started = performance.now();
    await within(api.refresh(), 5_000, "measured refresh");
    refreshSamples.push(performance.now() - started);
  }

  currentStage = "webview-load";
  if (!api.measureWebview) {
    throw new Error("the performance-only Webview measurement API is unavailable");
  }
  const framesPerSecond = numericEnvironment("RUNTROL_VSCODE_PERFORMANCE_RATE", 3_000);
  const durationMs = numericEnvironment("RUNTROL_VSCODE_PERFORMANCE_DURATION", 5_000);
  const webview = await within(
    api.measureWebview(framesPerSecond, durationMs),
    30_000,
    "Webview burst measurement",
  );
  const expectedFrames = Math.ceil(framesPerSecond * durationMs / 1_000);
  if (webview.producedFrames < expectedFrames) {
    throw new Error(`Webview load produced only ${webview.producedFrames} frames`);
  }
  if (webview.droppedFrames !== 0) {
    throw new Error(`Webview transport dropped ${webview.droppedFrames} raw frames`);
  }
  if (webview.visibleItems > 400 || webview.visibleCharacters > 256 * 1024) {
    throw new Error(
      `Webview bounds escaped at ${webview.visibleItems} items and ${webview.visibleCharacters} characters`,
    );
  }

  currentStage = "session-switch";
  if (!api.measureSessionManagement) {
    throw new Error("the performance-only hot-session measurement API is unavailable");
  }
  const switched = await within(
    api.measureSessionManagement(managedSessionIds()),
    20_000,
    "30-session management and eight hot session switches",
  );

  const result = {
    vscode: vscode.version,
    activationMs,
    openViewMs,
    refreshP95Ms: percentile(refreshSamples, 0.95),
    rssGrowthBytes: Math.max(0, process.memoryUsage().rss - rssBefore),
    webviewFrameP95Ms: webview.frameP95Ms,
    webviewBaselineFrameP95Ms: webview.baselineFrameP95Ms,
    webviewFrameOverrunP95Ms: webview.frameOverrunP95Ms,
    webviewInputP95Ms: webview.inputP95Ms,
    webviewScrollP95Ms: webview.scrollP95Ms,
    webviewPendingFrames: webview.maxPendingFrames,
    webviewDroppedFrames: webview.droppedFrames,
    sessionCount: switched.sessionCount,
    hotSessionCount: switched.hotSessionCount,
    coldResumeMs: switched.coldResumeMs,
    sessionSwitchP95Ms: switched.sessionSwitchP95Ms,
    resumedFrom: switched.resumedFrom,
    resumedTo: switched.resumedTo,
    restoreSession: switched.restoreSession,
    restoreWorkspace: switched.restoreWorkspace,
  };
  await writeFile(resultPath, JSON.stringify(result), "utf8");
  return result;
}

async function measureRestore(expected: string): Promise<{
  reloadRestoreMs: number;
  reloadActivationMs: number;
  reloadReadyMs: number;
  reloadViewMs: number;
  reloadSelectionMs: number;
}> {
  currentStage = "reload-activation";
  const extension = extensionUnderTest<ExtensionApi>();
  const started = performance.now();
  const api = await within(extension.activate() as Promise<ExtensionApi>, 5_000, "reload activation");
  const activatedAt = performance.now();
  let readyAt = activatedAt;
  let viewAt = activatedAt;
  const ready = within(api.ready, 5_000, "reload initialization").then(() => {
    readyAt = performance.now();
  });
  const view = (async () => {
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      5_000,
      "opening the Runtrol view after reload",
    );
    await within(
      vscode.commands.executeCommand("runtrol.conversation.focus"),
      5_000,
      "focusing the Runtrol Webview after reload",
    );
    viewAt = performance.now();
  })();
  await Promise.all([ready, view]);
  const readyAndViewAt = Math.max(readyAt, viewAt);
  if (!api.verifyRestoredSession) {
    throw new Error("the performance-only restored-session verifier is unavailable");
  }
  currentStage = "reload-selection";
  await within(api.verifyRestoredSession(expected), 5_000, "restoring the selected hot session");
  const selectedAt = performance.now();
  const reloadRestoreMs = selectedAt - started;
  const reloadActivationMs = activatedAt - started;
  const reloadReadyMs = readyAt - activatedAt;
  const reloadViewMs = viewAt - activatedAt;
  const reloadSelectionMs = selectedAt - readyAndViewAt;
  return {
    reloadRestoreMs,
    reloadActivationMs,
    reloadReadyMs,
    reloadViewMs,
    reloadSelectionMs,
  };
}

async function checkpoint(resultPath: string, stage: string): Promise<void> {
  currentStage = stage;
  await writeFile(resultPath, JSON.stringify({ stage }), "utf8");
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
    if (timer) {
      clearTimeout(timer);
    }
  }
}

function percentile(values: readonly number[], at: number): number {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * at) - 1] ?? Number.POSITIVE_INFINITY;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function numericEnvironment(name: string, fallback: number): number {
  const raw = process.env[name];
  const value = raw ? Number(raw) : fallback;
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${name} must be a positive number`);
  }
  return value;
}

function managedSessionIds(): string[] {
  const raw = requiredEnvironment("RUNTROL_VSCODE_MANAGED_SESSIONS");
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new Error("RUNTROL_VSCODE_MANAGED_SESSIONS is not JSON");
  }
  if (!Array.isArray(value) || value.length !== 30 || !value.every((item) => typeof item === "string")) {
    throw new Error("RUNTROL_VSCODE_MANAGED_SESSIONS must contain 30 session identifiers");
  }
  return value;
}
