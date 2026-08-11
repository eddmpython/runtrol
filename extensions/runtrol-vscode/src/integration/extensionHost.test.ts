import { writeFile } from "node:fs/promises";
import { performance } from "node:perf_hooks";

import * as vscode from "vscode";

import budget from "../../performance-budget.json";

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
    visibleCharacters: number;
    visibleItems: number;
  }>;
};

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await measure(resultPath);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        stage: currentStage,
      }),
      "utf8",
    );
    throw error;
  }
}

async function measure(resultPath: string): Promise<void> {
  const core = requiredEnvironment("RUNTROL_TEST_CORE");
  const configuredCore = vscode.workspace.getConfiguration("runtrol").get<string>("corePath");
  if (configuredCore !== core) {
    throw new Error(`the isolated VS Code profile configured unexpected Core ${String(configuredCore)}`);
  }
  const extension = vscode.extensions.getExtension("eddmpython.runtrol-studio");
  if (!extension) {
    throw new Error("the Runtrol Studio development extension is missing");
  }
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
  if (webview.visibleItems > 400 || webview.visibleCharacters > 256 * 1024) {
    throw new Error(
      `Webview bounds escaped at ${webview.visibleItems} items and ${webview.visibleCharacters} characters`,
    );
  }

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
  };
  await writeFile(resultPath, JSON.stringify(result), "utf8");

  const failures = budgetFailures(result);
  if (failures.length > 0) {
    throw new Error(failures.join("; "));
  }
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

function budgetFailures(result: {
  activationMs: number;
  openViewMs: number;
  refreshP95Ms: number;
  rssGrowthBytes: number;
  webviewFrameP95Ms: number;
  webviewFrameOverrunP95Ms: number;
  webviewInputP95Ms: number;
  webviewScrollP95Ms: number;
  webviewPendingFrames: number;
}): string[] {
  const failures: string[] = [];
  if (result.activationMs > budget.activationMs) {
    failures.push(`activation ${result.activationMs.toFixed(1)} ms exceeds ${budget.activationMs} ms`);
  }
  if (result.openViewMs > budget.openViewMs) {
    failures.push(`view open ${result.openViewMs.toFixed(1)} ms exceeds ${budget.openViewMs} ms`);
  }
  if (result.refreshP95Ms > budget.refreshP95Ms) {
    failures.push(`refresh p95 ${result.refreshP95Ms.toFixed(1)} ms exceeds ${budget.refreshP95Ms} ms`);
  }
  if (result.rssGrowthBytes > budget.rssGrowthBytes) {
    failures.push(`RSS growth ${result.rssGrowthBytes} exceeds ${budget.rssGrowthBytes} bytes`);
  }
  if (result.webviewFrameP95Ms > budget.webviewFrameP95Ms) {
    failures.push(
      `Webview frame p95 ${result.webviewFrameP95Ms.toFixed(1)} ms exceeds ${budget.webviewFrameP95Ms} ms`,
    );
  }
  if (result.webviewFrameOverrunP95Ms > budget.webviewFrameOverrunP95Ms) {
    failures.push(
      `Webview frame overrun p95 ${result.webviewFrameOverrunP95Ms.toFixed(1)} ms exceeds `
      + `${budget.webviewFrameOverrunP95Ms} ms`,
    );
  }
  if (result.webviewInputP95Ms > budget.webviewInputP95Ms) {
    failures.push(
      `Webview input p95 ${result.webviewInputP95Ms.toFixed(1)} ms exceeds ${budget.webviewInputP95Ms} ms`,
    );
  }
  if (result.webviewScrollP95Ms > budget.webviewScrollP95Ms) {
    failures.push(
      `Webview scroll p95 ${result.webviewScrollP95Ms.toFixed(1)} ms exceeds ${budget.webviewScrollP95Ms} ms`,
    );
  }
  if (result.webviewPendingFrames > budget.webviewPendingFrames) {
    failures.push(
      `Webview pending frames ${result.webviewPendingFrames} exceeds ${budget.webviewPendingFrames}`,
    );
  }
  return failures;
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
