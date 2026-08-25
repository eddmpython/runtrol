/// The window-facing half of Core supersession: quiet success, one toast for the busy case, and
/// one human-confirmed button for a daemon too old to interrogate. The decision itself lives in
/// coreSupersession.ts, which knows nothing about windows and is unit-tested there.

import { execFile } from "node:child_process";

import * as vscode from "vscode";

import type { CoreClient } from "./core/client";
import type { CoreLocator } from "./core/locator";
import { managedCoreDirectory } from "./core/managedCore";
import {
  ensureCurrentCore,
  retryWhileTheOldDaemonExits,
  type SupersessionOutcome,
} from "./coreSupersession";

const BUSY_RETRY_MS = 60_000;


/// Keep the running daemon on the installed build for the life of the window.
///
/// Only the first attempt is awaited, so activation never waits on a busy machine: a window must
/// open even while an older daemon keeps serving running conversations, because the hello stays
/// readable across builds either way. The busy case retries quietly in the background.
export async function superviseCoreCurrency(client: CoreClient, locator: CoreLocator): Promise<void> {
  const outcome = await attemptOnce(client, locator);
  if (outcome === null || outcome.state === "current" || outcome.state === "superseded") {
    return;
  }
  if (outcome.state === "legacy") {
    await offerManualCoreRestart(client, locator, outcome.detail);
    return;
  }
  void vscode.window.showInformationMessage(
    "The Runtrol update applies automatically when the agents working right now finish their turns.",
  );
  void retryUntilIdle(client, locator);
}

/// One supersession attempt, with a connection failure treated as nothing to roll.
async function attemptOnce(client: CoreClient, locator: CoreLocator): Promise<SupersessionOutcome | null> {
  try {
    return await ensureCurrentCore(client, () => locator.managedDigest());
  } catch {
    // ok: connecting failed entirely; whoever needs the daemon next reports that failure with its
    // own words, and rolling a daemon we cannot reach is not a thing.
    return null;
  }
}

/// The background half of the busy case: ask again each minute until the machine goes idle.
async function retryUntilIdle(client: CoreClient, locator: CoreLocator): Promise<void> {
  for (;;) {
    await new Promise((resolve) => setTimeout(resolve, BUSY_RETRY_MS));
    const outcome = await attemptOnce(client, locator);
    if (outcome === null || outcome.state !== "busy") {
      if (outcome?.state === "legacy") {
        await offerManualCoreRestart(client, locator, outcome.detail);
      }
      return;
    }
  }
}

/// One button for the daemon this build cannot interrogate: stop it by exact process identity.
///
/// Only a person confirms replacing a build that predates the retire request, and only the
/// extension's own installed executable is ever touched: processes are matched by the exact
/// managed Core path, never by name.
async function offerManualCoreRestart(
  client: CoreClient,
  locator: CoreLocator,
  detail: string,
): Promise<void> {
  // `detail` is the old daemon's own refusal, which for a pre-retire build is its list of
  // commands. It stays out of the sentence: a person deciding whether to press a button needs
  // what will happen, not the vocabulary of a program they never asked about. This extension logs
  // nowhere by design, so the reason travels only when the restart itself fails, where it helps.
  //
  // Stated plainly rather than hedged. Unlike managed retirement, which refuses while any
  // conversation has a live process, this path cannot ask an older daemon what it is running, so
  // the person pressing the button is the one who knows whether an agent is mid-turn.
  const restart = "Restart the Runtrol Core";
  const picked = await vscode.window.showWarningMessage(
    "An older Runtrol Core is still running, so the Runtrol update has not taken effect yet. "
      + "Restarting it applies the update: conversations reopen from their saved state, and any "
      + "agent that is working right now stops mid-turn.",
    restart,
  );
  if (picked !== restart) return;
  try {
    const { executable } = await locator.locate();
    // The old daemon runs from the previous content-named image in the same managed directory (or the
    // single-name image an older extension installed there), never from the new file: the directory is
    // the identity to match, still never the process name.
    await stopManagedCoreProcesses(managedCoreDirectory(executable));
    await client.reset();
    await retryWhileTheOldDaemonExits(client);
    void vscode.window.showInformationMessage("The Runtrol Core is now on the installed build.");
  } catch (error) {
    void vscode.window.showErrorMessage(
      `Restarting the Runtrol Core failed: ${error instanceof Error ? error.message : String(error)}`
        + ` (the running Core reported: ${detail})`,
    );
  }
}

/// Stop every process running an image inside this extension's managed Core directory. Windows-precise;
/// elsewhere the legacy daemon predates this build's platforms of record and the failure names the
/// manual step.
function stopManagedCoreProcesses(directory: string): Promise<void> {
  if (process.platform !== "win32") {
    return Promise.reject(new Error(
      `stop the daemon running from ${directory} manually, then run any Runtrol action to start the new one`,
    ));
  }
  // Inside a PowerShell single-quoted literal only the quote itself needs doubling; backslashes
  // are literal characters there, and doubling them would make the path match nothing.
  const escaped = `${directory.replace(/[\\/]+$/u, "")}\\`.replaceAll("'", "''");
  // Case-insensitive on purpose: measured 2026-08-20, the same daemon was reported as both `C:\...`
  // and `c:\...` depending on who asked, and VS Code's own fsPath can lower the drive letter. Identity
  // here is the directory, never the process name: `Name='runtrol.exe'` only narrows the scan, and a
  // build somewhere else on disk (the operator's own, another checkout's) must survive this untouched.
  const script =
    `Get-CimInstance Win32_Process -Filter "Name='runtrol.exe'" | ` +
    `Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith('${escaped}', [System.StringComparison]::OrdinalIgnoreCase) } | ` +
    `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -Confirm:$false }`;
  return new Promise((resolve, reject) => {
    execFile(
      "powershell",
      ["-NoProfile", "-NonInteractive", "-Command", script],
      { timeout: 15_000, windowsHide: true },
      (error, _stdout, stderr) => {
        if (error) reject(new Error(stderr.trim() || error.message));
        else resolve();
      },
    );
  });
}
