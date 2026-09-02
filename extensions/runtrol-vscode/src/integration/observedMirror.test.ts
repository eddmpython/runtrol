import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import type { RuntrolExtensionApi } from "../extension";
import type { MirrorEvidence } from "../windowRegistry";
import { extensionUnderTest } from "./extensionUnderTest.test";

/// The observed-mirror journey inside one isolated window (`EXT-02`, driven by `tooling/observed-mirror-eye.mjs`):
/// the harness names a program to run in an ordinary terminal; this window runs it through shell integration, so
/// Studio's registry sees the execution start and mirrors it on its own; the window then reports the mirror's
/// evidence, waits for the harness to attach a viewer, sends the exit keys, and reports the final evidence.
///
/// Coordination is files in one folder: the harness publishes `<role>-run-<n>.json` and `<role>-exit-<n>.json`,
/// the window publishes `<role>-ready.json`, `<role>-mirror-<n>.json` and `<role>-ended-<n>.json`.
const DEADLINE_MS = 60_000;

type Run = {
  readonly done?: boolean;
  readonly label: string;
  readonly commandLine: string;
  readonly exitKeys: readonly string[];
  readonly exitKeyGapMs: number;
};

export async function run(): Promise<void> {
  const coordination = requiredEnvironment("RUNTROL_VSCODE_COORDINATION");
  const role = requiredEnvironment("RUNTROL_VSCODE_ROLE");
  await mkdir(coordination, { recursive: true });
  try {
    await journey(coordination, role);
  } catch (error) {
    await publish(coordination, `${role}-failure.json`, {
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack ?? null : null,
    });
    throw error;
  }
}

async function journey(coordination: string, role: string): Promise<void> {
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  await within(api.ready, DEADLINE_MS, "extension readiness");
  if (!api.journey) throw new Error("the journey API is unavailable");
  const journey = api.journey;
  await publish(coordination, `${role}-ready.json`, { sessionId: vscode.env.sessionId, hostPid: process.pid });
  // The eye looks at the sidebar and the terminal, so both are on screen the way a person has them.
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");

  for (let index = 1; ; index += 1) {
    const step = await readPublished<Run>(coordination, `${role}-run-${index}.json`, DEADLINE_MS * 5);
    if (step.done) break;
    const before = journey.windowMirrors().length;
    const terminal = vscode.window.createTerminal({ name: `${role}-${step.label}` });
    terminal.show(false);
    await terminal.processId;
    if (!(await waitForShellIntegration(terminal, 30_000))) throw new Error(`${step.label}: shell integration never attached`);
    const ended = new Promise<number | null | undefined>((resolve) => {
      const subscription = vscode.window.onDidEndTerminalShellExecution((end) => {
        if (end.terminal !== terminal) return;
        subscription.dispose();
        resolve(end.exitCode);
      });
    });
    let startedCommandLine: string | null = null;
    const startWatch = vscode.window.onDidStartTerminalShellExecution((start) => {
      if (start.terminal === terminal) startedCommandLine = start.execution.commandLine.value;
    });
    terminal.shellIntegration?.executeCommand(step.commandLine);
    let opened: MirrorEvidence;
    try {
      opened = await waitForMirror(journey, before, 30_000);
    } catch (error) {
      const providers = journey.providers().map((provider) => ({ id: provider.providerId, names: provider.commandNames ?? [], state: provider.installation.state }));
      throw new Error(`${error instanceof Error ? error.message : String(error)}; started=${JSON.stringify(startedCommandLine)} cwd=${JSON.stringify(terminal.shellIntegration?.cwd?.fsPath ?? null)} known=${JSON.stringify(journey.windowCommandNames())} providers=${JSON.stringify(providers)} mirrors=${JSON.stringify(journey.windowMirrors())}`);
    } finally {
      startWatch.dispose();
    }
    await publish(coordination, `${role}-mirror-${index}.json`, { ...opened, terminalName: terminal.name });

    await readPublished(coordination, `${role}-exit-${index}.json`, DEADLINE_MS);
    // Each exit key gets its gap to take effect; the next one is sent only while the command still runs.
    let exitCode: number | null | undefined | "timeout" = "timeout";
    for (const key of step.exitKeys) {
      terminal.sendText(key, false);
      exitCode = await Promise.race([ended, delay(step.exitKeyGapMs).then(() => "timeout" as const)]);
      if (exitCode !== "timeout") break;
    }
    if (exitCode === "timeout") exitCode = await Promise.race([ended, delay(15_000).then(() => "timeout" as const)]);
    if (exitCode === "timeout") terminal.dispose();
    // The pump feeds the last chunks and the end after the event; give it a moment to settle.
    const final = await waitForMirrorEnd(journey, opened.executionId, 10_000);
    await publish(coordination, `${role}-ended-${index}.json`, {
      ...final,
      shellExitCode: exitCode === "timeout" ? null : exitCode ?? null,
      timedOut: exitCode === "timeout",
    });
  }
}

async function waitForMirror(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  before: number,
  deadlineMs: number,
): Promise<MirrorEvidence> {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const mirrors = journey.windowMirrors();
    const latest = mirrors[mirrors.length - 1];
    if (mirrors.length > before && latest && (latest.terminalId !== null || latest.refusal !== null)) return latest;
    await delay(25);
  }
  throw new Error("no mirror was opened or refused for the command");
}

async function waitForMirrorEnd(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  executionId: string,
  deadlineMs: number,
): Promise<MirrorEvidence> {
  const deadline = Date.now() + deadlineMs;
  let last: MirrorEvidence | undefined;
  while (Date.now() < deadline) {
    last = journey.windowMirrors().find((mirror) => mirror.executionId === executionId);
    if (last?.ended) {
      // One more settle so the pump's final chunk and end land before the evidence is read.
      await delay(1_000);
      return journey.windowMirrors().find((mirror) => mirror.executionId === executionId) ?? last;
    }
    await delay(50);
  }
  if (!last) throw new Error("the mirror evidence disappeared");
  return last;
}

async function waitForShellIntegration(terminal: vscode.Terminal, deadlineMs: number): Promise<boolean> {
  const started = Date.now();
  while (Date.now() - started < deadlineMs) {
    if (terminal.shellIntegration?.cwd) return true;
    await delay(50);
  }
  return false;
}

async function publish(coordination: string, name: string, value: unknown): Promise<void> {
  const finalPath = path.join(coordination, name);
  const temporary = `${finalPath}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(value), "utf8");
  await rename(temporary, finalPath);
}

async function readPublished<T = Record<string, unknown>>(coordination: string, name: string, deadlineMs: number): Promise<T> {
  const file = path.join(coordination, name);
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    try {
      const value: unknown = JSON.parse(await readFile(file, "utf8"));
      if (value && typeof value === "object" && !Array.isArray(value)) return value as T;
      throw new Error(`${name} is not a JSON object`);
    } catch (error) {
      if (!(error instanceof SyntaxError) && (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    await delay(25);
  }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms`);
}

function within<T>(work: Promise<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
