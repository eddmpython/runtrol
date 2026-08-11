import { writeFile } from "node:fs/promises";
import { performance } from "node:perf_hooks";

import * as vscode from "vscode";

import budget from "../../performance-budget.json";

let currentStage = "starting";

type ExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
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

  const viewStarted = performance.now();
  await checkpoint(resultPath, "view-opening");
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

  const result = {
    vscode: vscode.version,
    activationMs,
    openViewMs,
    refreshP95Ms: percentile(refreshSamples, 0.95),
    rssGrowthBytes: Math.max(0, process.memoryUsage().rss - rssBefore),
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
