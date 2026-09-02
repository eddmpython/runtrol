// The `vscode:uninstall` hook: what Studio takes away when the operator removes it.
//
// VS Code runs this file with its own Electron as plain Node on the start after the extension was uninstalled
// (measured 2026-09-02 on 1.132.1: the second start, as `Code.exe <this file> --type=extension-post-uninstall`,
// with the extension folder still present, the VS Code process environment, and no VS Code API). Nothing here
// imports `vscode`.
//
// What it removes is Runtrol's own residue and nothing else: the managed Core images, provider shims, and other
// Studio storage under the extension's global storage, the daemons running from those images, and the Runtime
// state root when no standalone Runtime install owns it. Provider profiles, provider processes that Runtrol never
// started, and provider-owned conversations are never read or touched.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";

export const UNINSTALL_RECORD = "uninstall.json";
export const EXTENSION_IDENTIFIER = "runtrol.runtrol-studio";

/// Written by Studio at every activation, beside this hook, so the hook knows which global storage it owned.
export type UninstallRecord = {
  schema: 1;
  globalStorage: string;
};

export type Environment = Record<string, string | undefined>;

export type Plan = {
  /// Studio's global storage directory, or null when neither the record nor a default location names one.
  globalStorage: string | null;
  /// The Runtime state root this Studio used.
  stateRoot: string;
  /// Whether the state root is Studio's to remove: no standalone Runtime install shares it.
  removeStateRoot: boolean;
};

/// The Runtime state root exactly as the Core resolves it: the operator's `RUNTROL_HOME` first, else the
/// platform directory.
export function stateRootOf(env: Environment, platform: NodeJS.Platform = process.platform, home = os.homedir()): string {
  if (env.RUNTROL_HOME) return env.RUNTROL_HOME;
  if (platform === "win32") return path.join(env.LOCALAPPDATA ?? path.join(home, "AppData", "Local"), "runtrol");
  if (platform === "darwin") return path.join(home, "Library", "Application Support", "runtrol");
  return path.join(env.XDG_STATE_HOME ?? path.join(home, ".local", "state"), "runtrol");
}

/// Where the standalone Runtime installer keeps its versioned executables; when it exists the state root is
/// shared and stays.
export function standaloneRootOf(env: Environment, platform: NodeJS.Platform = process.platform, home = os.homedir()): string {
  if (platform === "win32") return path.join(env.LOCALAPPDATA ?? path.join(home, "AppData", "Local"), "RuntrolRuntime");
  return path.join(home, ".local", "share", "runtrol");
}

/// Default global storage locations, for an install whose record could not be written (a read-only extension
/// folder) or read.
export function defaultGlobalStorages(env: Environment, platform: NodeJS.Platform = process.platform, home = os.homedir()): string[] {
  const userRoots = platform === "win32"
    ? ["Code", "Code - Insiders"].map((product) => path.join(env.APPDATA ?? path.join(home, "AppData", "Roaming"), product, "User"))
    : platform === "darwin"
      ? ["Code", "Code - Insiders"].map((product) => path.join(home, "Library", "Application Support", product, "User"))
      : ["Code", "Code - Insiders"].map((product) => path.join(env.XDG_CONFIG_HOME ?? path.join(home, ".config"), product, "User"));
  return userRoots.map((root) => path.join(root, "globalStorage", EXTENSION_IDENTIFIER));
}

export function readRecord(hookDirectory: string, read: (file: string) => string = (file) => readFileSync(file, "utf8")): UninstallRecord | null {
  let text: string;
  try {
    text = read(path.join(hookDirectory, UNINSTALL_RECORD));
  } catch {
    // ok: a missing record is the read-only-install case the default locations below exist for; the hook
    // continues with what it can prove rather than stopping on the absence.
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    // ok: same as above; a record that is not JSON is treated as absent, never as a path to delete.
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const { schema, globalStorage } = parsed as { schema?: unknown; globalStorage?: unknown };
  if (schema !== 1 || typeof globalStorage !== "string" || !path.isAbsolute(globalStorage)) return null;
  if (path.basename(globalStorage) !== EXTENSION_IDENTIFIER) return null;
  return { schema: 1, globalStorage };
}

export type RunningProcess = { pid: number; executable: string };

/// The global storage a running Studio daemon proves: its executable lives in `<storage>/core/`.
///
/// The second source of truth after the record. A daemon started by Studio runs from the managed Core directory
/// inside Studio's global storage, so the locator's process ids, joined with the process list, name that
/// storage even when no record could be written.
export function storageFromDaemons(locatorText: string | null, processes: readonly RunningProcess[]): string | null {
  if (!locatorText) return null;
  let generations: unknown;
  try {
    generations = (JSON.parse(locatorText) as { generations?: unknown }).generations;
  } catch {
    // ok: an unreadable locator proves nothing, and nothing is derived from a guess.
    return null;
  }
  if (!Array.isArray(generations)) return null;
  const pids = new Set(generations
    .map((generation) => (generation as { processId?: unknown }).processId)
    .filter((pid): pid is number => typeof pid === "number"));
  for (const candidate of processes) {
    if (!pids.has(candidate.pid)) continue;
    const parts = path.resolve(candidate.executable).split(path.sep);
    const at = parts.findIndex((part, index) =>
      part.toLowerCase() === EXTENSION_IDENTIFIER && parts[index - 1]?.toLowerCase() === "globalstorage" && parts[index + 1]?.toLowerCase() === "core");
    if (at > 0) return parts.slice(0, at + 1).join(path.sep);
  }
  return null;
}

/// Decide what to remove from the record, the running daemons, the environment, and what exists on disk.
export function plan(
  record: UninstallRecord | null,
  env: Environment,
  exists: (file: string) => boolean,
  platform: NodeJS.Platform = process.platform,
  home = os.homedir(),
  provedByDaemons: string | null = null,
): Plan {
  const globalStorage = record?.globalStorage
    ?? provedByDaemons
    ?? defaultGlobalStorages(env, platform, home).find((candidate) => exists(candidate))
    ?? null;
  const stateRoot = stateRootOf(env, platform, home);
  return {
    globalStorage,
    stateRoot,
    removeStateRoot: !exists(standaloneRootOf(env, platform, home)),
  };
}

/// The daemons that run from Studio's managed Core directory, and only those: a standalone Runtime, or a
/// development build, runs from somewhere else and is not Studio's to stop.
export function managedDaemons(managedCore: string, processes: readonly RunningProcess[]): RunningProcess[] {
  const prefix = path.resolve(managedCore).toLowerCase() + path.sep;
  return processes.filter((process) => path.resolve(process.executable).toLowerCase().startsWith(prefix));
}

function runningProcesses(): RunningProcess[] {
  if (process.platform !== "win32") {
    const listed = spawnSync("ps", ["-axo", "pid=,comm="], { encoding: "utf8", timeout: 15_000 });
    return (listed.stdout ?? "")
      .split("\n")
      .map((line) => line.trim().match(/^(\d+)\s+(.+)$/u))
      .filter((match): match is RegExpMatchArray => match !== null)
      .map((match) => ({ pid: Number(match[1]), executable: match[2] ?? "" }));
  }
  const listed = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command",
      "Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath } | Select-Object ProcessId, ExecutablePath | ConvertTo-Json -Compress"],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  let parsed: unknown;
  try {
    parsed = JSON.parse(listed.stdout || "[]");
  } catch {
    // ok: a process list that cannot be read means nothing can be proved to be ours, so nothing is stopped and
    // the removal below fails visibly on the still-mapped image instead of guessing at a pid.
    return [];
  }
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  return rows
    .filter((row): row is { ProcessId: number; ExecutablePath: string } =>
      Boolean(row) && typeof (row as { ProcessId?: unknown }).ProcessId === "number"
      && typeof (row as { ExecutablePath?: unknown }).ExecutablePath === "string")
    .map((row) => ({ pid: row.ProcessId, executable: row.ExecutablePath }));
}

function sleep(ms: number): void {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    // Busy waiting is acceptable for a one-off hook that runs once per uninstall and holds nothing else.
  }
}

/// Stop every daemon running from the managed Core directory through the product's own panic button, then
/// end any that is still there by its exact pid.
function stopManagedDaemons(managedCore: string): void {
  const first = managedDaemons(managedCore, runningProcesses());
  if (first.length === 0) return;
  spawnSync(first[0]!.executable, ["panic"], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (managedDaemons(managedCore, runningProcesses()).length === 0) return;
    sleep(500);
  }
  for (const remaining of managedDaemons(managedCore, runningProcesses())) {
    if (process.platform === "win32") {
      spawnSync("taskkill.exe", ["/PID", String(remaining.pid), "/T", "/F"], { encoding: "utf8", timeout: 15_000, windowsHide: true });
    } else {
      process.kill(remaining.pid, "SIGKILL");
    }
  }
}

function removeTree(directory: string, failures: string[]): void {
  try {
    rmSync(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  } catch (error) {
    failures.push(`${directory}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function locatorText(stateRoot: string): string | null {
  try {
    return readFileSync(path.join(stateRoot, "runtime.locator.json"), "utf8");
  } catch {
    // ok: no locator means no daemon this hook could learn a storage path from; the record and the default
    // locations remain the sources.
    return null;
  }
}

/// The hook body. `hookDirectory` is `dist/`, where VS Code runs this file; the record sits one level up, in the
/// extension folder beside `package.json`.
export function main(hookDirectory = __dirname, env: Environment = process.env): number {
  const stateRoot = stateRootOf(env);
  const processes = runningProcesses();
  const decided = plan(
    readRecord(path.resolve(hookDirectory, "..")),
    env,
    existsSync,
    process.platform,
    os.homedir(),
    storageFromDaemons(locatorText(stateRoot), processes),
  );
  const failures: string[] = [];
  if (decided.globalStorage) {
    stopManagedDaemons(path.join(decided.globalStorage, "core"));
    removeTree(decided.globalStorage, failures);
  }
  if (decided.removeStateRoot) {
    removeTree(decided.stateRoot, failures);
  }
  for (const failure of failures) {
    process.stderr.write(`runtrol uninstall left: ${failure}
`);
  }
  return failures.length === 0 ? 0 : 1;
}

if (require.main === module) {
  process.exitCode = main();
}
