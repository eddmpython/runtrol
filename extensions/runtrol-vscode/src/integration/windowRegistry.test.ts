import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import type { RuntrolExtensionApi } from "../extension";
import { extensionUnderTest } from "./extensionUnderTest.test";

/// The in-window half of the window registry journey (`tooling/window-registry-eye.mjs`, `EXT-01`).
///
/// The extension registers this window with the Runtime on its own; this entry only does what a person would:
/// open terminals, close one, run a command in one, and restart the Extension Host when told. The harness outside
/// reads the Runtime's registry between the steps through the public wire.
const DEADLINE_MS = 60_000;

export async function run(): Promise<void> {
  const coordination = requiredEnvironment("RUNTROL_VSCODE_COORDINATION");
  const role = requiredEnvironment("RUNTROL_VSCODE_ROLE");
  await mkdir(coordination, { recursive: true });
  try {
    await journey(coordination, role);
  } catch (error) {
    await publish(coordination, `${role}-failure.json`, {
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    });
    throw error;
  }
}

async function journey(coordination: string, role: string): Promise<void> {
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  await within(api.ready, DEADLINE_MS, "extension readiness");
  await publish(coordination, `${role}-ready.json`, { sessionId: vscode.env.sessionId, hostPid: process.pid });

  await readPublished(coordination, `${role}-open.json`, DEADLINE_MS);
  const one = vscode.window.createTerminal({ name: `${role}-one` });
  const two = vscode.window.createTerminal({ name: `${role}-two` });
  await Promise.all([one.processId, two.processId]);
  await publish(coordination, `${role}-opened.json`, { one: await one.processId, two: await two.processId });

  const command = await readPublished<{ run: boolean }>(coordination, `${role}-command.json`, DEADLINE_MS);
  if (command.run) {
    await waitForShellIntegration(one, 20_000);
    const started = new Promise<string | null>((resolve) => {
      const subscription = vscode.window.onDidStartTerminalShellExecution((start) => {
        if (start.terminal === one) {
          subscription.dispose();
          resolve(start.execution.commandLine.value);
        }
      });
      setTimeout(() => { subscription.dispose(); resolve(null); }, 20_000);
    });
    one.shellIntegration?.executeCommand("echo registry-command-one");
    await publish(coordination, `${role}-commanded.json`, { commandLine: await started });
  }

  await readPublished(coordination, `${role}-close.json`, DEADLINE_MS);
  two.dispose();
  await delay(500);
  await publish(coordination, `${role}-closed.json`, {});

  const restart = await readPublished<{ restart: boolean }>(coordination, `${role}-finish.json`, DEADLINE_MS);
  if (restart.restart) {
    // The host ends here; the extension activates again in the new host and registers this same window anew.
    await vscode.commands.executeCommand("workbench.action.restartExtensionHost");
  }
}

async function waitForShellIntegration(terminal: vscode.Terminal, deadlineMs: number): Promise<boolean> {
  const started = Date.now();
  while (Date.now() - started < deadlineMs) {
    if (terminal.shellIntegration) return true;
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
