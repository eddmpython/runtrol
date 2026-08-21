import { execFile } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";

type ExtensionApi = { readonly ready: Promise<void> };

/// The focused Agent Tools eye pass.
///
/// The outer harness gives both provider CLIs clean configuration homes and an isolated Runtime. This entry then
/// uses the shipped VS Code commands, reads the shipped Core's persisted status, and photographs the project row.
/// It starts no provider session and sends no model turn.
export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  const folder = path.resolve(requiredEnvironment("RUNTROL_EYE_FOLDER"));
  const core = requiredEnvironment("RUNTROL_TEST_CORE");
  let enabled = false;
  try {
    const extension = extensionUnderTest<ExtensionApi>();
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      30_000,
      "opening the Runtrol view",
    );
    while (!extension.isActive) await delay(25);
    await within(extension.exports.ready, 60_000, "extension initialization");
    await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
    await delay(1_000);

    await within(
      vscode.commands.executeCommand("runtrol.enableAgentTools"),
      180_000,
      "enabling Agent Tools from VS Code",
    );
    await waitForStatus(core, folder, true, 30_000);
    enabled = true;
    await delay(1_500);
    await capture(resultPath, "agentToolsEnabled", {
      project: folder,
      status: "enabled",
      providerConfiguration: "isolated",
    });

    await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
    await within(
      vscode.commands.executeCommand("runtrol.disableAgentTools"),
      180_000,
      "disabling Agent Tools from VS Code",
    );
    await waitForStatus(core, folder, false, 30_000);
    enabled = false;
    await delay(1_500);
    await capture(resultPath, "agentToolsDisabled", {
      project: folder,
      status: "disabled",
      authority: "revoked",
    });

    await writeFile(resultPath, JSON.stringify({
      stage: "complete",
      project: folder,
      enabledThenRevoked: true,
      modelTurns: 0,
    }), "utf8");
  } catch (error) {
    if (enabled) {
      await vscode.commands.executeCommand("runtrol.disableAgentTools").then(undefined, () => undefined);
    }
    await writeFile(resultPath, JSON.stringify({
      stage: "failed",
      failure: error instanceof Error ? error.message : String(error),
    }), "utf8");
    throw error;
  }
}

async function waitForStatus(
  core: string,
  folder: string,
  expected: boolean,
  deadlineMs: number,
): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    const output = await runCore(core, ["tools", "list"], folder);
    const lines = output.split(/\r?\n/u).map((line) => line.trim()).filter(Boolean);
    const enabled = lines.some((line) => line === `enabled  ${folder}`);
    const disabled = lines.length === 1 && lines[0] === "no projects enabled";
    if ((expected && enabled) || (!expected && disabled)) return;
    if (Date.now() > deadline) {
      throw new Error(`Agent Tools status did not become ${expected ? "enabled" : "disabled"}: ${lines.join(" | ")}`);
    }
    await delay(100);
  }
}

function runCore(executable: string, words: readonly string[], cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      [...words],
      { cwd, encoding: "utf8", timeout: 30_000, windowsHide: true },
      (error, stdout, stderr) => {
        if (error) reject(new Error(`${words.join(" ")} failed: ${stderr || error.message}`));
        else resolve(`${stdout}\n${stderr}`);
      },
    );
  });
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}`, ...facts }), "utf8");
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await readFile(confirmation, "utf8");
      return;
    } catch {
      // The outer photographer has not captured this pose yet.
    }
    if (Date.now() > deadline) throw new Error(`the harness never confirmed the ${pose} capture`);
    await delay(250);
  }
}

function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    work.then(
      (value) => {
        clearTimeout(timeout);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timeout);
        reject(error);
      },
    );
  });
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
