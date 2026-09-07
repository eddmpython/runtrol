import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import * as vscode from "vscode";
import { ProviderAccountActions } from "../providerAccountAction";

/// A real editor task journey with synthetic commands only. No provider or account environment is touched.
export async function run(): Promise<void> {
  const result = process.env.RUNTROL_ACCOUNT_ACTION_RESULT;
  if (!result) throw new Error("RUNTROL_ACCOUNT_ACTION_RESULT is required");
  let refreshes = 0;
  let completed!: () => void;
  const completion = new Promise<void>((resolve) => { completed = resolve; });
  const actions = new ProviderAccountActions(() => { refreshes += 1; completed(); });
  const ended: vscode.TaskExecution[] = [];
  const listener = vscode.tasks.onDidEndTaskProcess(({ execution }) => { ended.push(execution); });
  const command = (seconds: number) => process.platform === "win32"
    ? `powershell.exe -NoProfile -NonInteractive -Command "Write-Output 'Provider account completion fixture'; Start-Sleep -Seconds ${seconds}"`
    : `printf 'Provider account completion fixture\\n'; sleep ${seconds}`;
  try {
    const unrelated = await vscode.tasks.executeTask(new vscode.Task(
      { type: "runtrol-account", provider: "unrelated", token: "fixture" }, vscode.TaskScope.Global, "Unrelated fixture", "Runtrol",
      new vscode.ShellExecution(command(0)), [],
    ));
    await within(new Promise<void>((resolve) => {
      if (ended.includes(unrelated)) return resolve();
      const watch = vscode.tasks.onDidEndTaskProcess(({ execution }) => {
        if (execution !== unrelated) return;
        watch.dispose(); resolve();
      });
    }));
    assert.equal(refreshes, 0);
    await actions.run("account-fixture", "1.0.0", "Provider account completion", command(12));
    await writeFile(`${result}.ready`, JSON.stringify({ hostPid: process.pid, refreshes, vscode: vscode.version }));
    await within(completion);
    assert.equal(refreshes, 1);
    assert.equal(ended.length, 2);
    await writeFile(result, JSON.stringify({ refreshes, unrelatedRefreshes: 0,
      exactExecutions: ended.length, vscode: vscode.version }));
    await within((async () => {
      for (;;) {
        try { await readFile(`${result}.captured`); return; } catch {
          // The external Win32 capture owns this rendezvous; no provider or product polling is involved.
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
    })());
  } finally {
    listener.dispose(); actions.dispose();
  }
}

async function within(promise: Promise<void>): Promise<void> {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([promise, new Promise<never>((_resolve, reject) => {
      timeout = setTimeout(() => reject(new Error("Account task completion timed out")), 30_000);
    })]);
  } finally {
    if (timeout) clearTimeout(timeout);
  }
}
