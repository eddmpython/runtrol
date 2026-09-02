// The window registry journey (`EXT-01`): two isolated real VS Code windows on one isolated Runtime. Each window's
// Studio registers itself and the terminals it observes on its own; this harness has the windows open terminals,
// run a command, close a terminal, and restart one Extension Host, and reads the Runtime's registry between the
// steps through the public wire. Prints one `RUNTROL_WINDOW_REGISTRY {json}` line.
//
// Usage: node tooling/window-registry-eye.mjs
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  acquireVSCode,
  isolatedExtensionTestArguments,
  isolatedLaunchArguments,
  isolatedProfileSettings,
  isolatedRuntimeState,
  terminateExactProcesses,
} from "./isolated-vscode.mjs";

const MARKER = "RUNTROL_WINDOW_REGISTRY ";
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const executionRoot = process.platform === "win32"
  ? path.join(requiredEnvironment("LOCALAPPDATA"), "dev-workspace")
  : path.join(os.homedir(), ".local", "share", "dev-workspace");
await mkdir(executionRoot, { recursive: true });
const suffix = process.platform === "win32" ? ".exe" : "";
const target = path.join(repositoryRoot, "target", "debug");
const core = path.join(target, `runtrol${suffix}`);
const probe = path.join(target, "examples", `handoverProbe${suffix}`);
for (const [packageName, kind, name] of [["runtrol", "--bin", "runtrol"], ["runtrol", "--example", "handoverProbe"]]) {
  const built = spawnSync("cargo", ["build", "-p", packageName, kind, name], { cwd: repositoryRoot, encoding: "utf8", windowsHide: true });
  if (built.status !== 0) throw new Error(`cargo build ${name} failed:\n${built.stderr}`);
}
await Promise.all([stat(core), stat(probe)]);
const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-winreg-"));
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "windowRegistry.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };

let daemon = null;
const windows = [];
let executable = null;
let generationDigest = null;
try {
  await mkdir(coordination, { recursive: true });
  await mkdir(runtrolHome, { recursive: true });
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "windowRegistry.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });
  daemon = spawn(core, ["daemon"], { env: daemonEnvironment, stdio: "ignore", windowsHide: true });
  await delay(500);
  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));

  const roles = ["alpha", "beta"];
  for (const role of roles) {
    const userData = path.join(temporary, `${role}-user`);
    const extensions = path.join(temporary, `${role}-extensions`);
    const workspace = path.join(temporary, `${role}-workspace`);
    await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
    await writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core }),
      "utf8",
    );
    const child = spawn(
      executable,
      isolatedExtensionTestArguments({ workspace, userData, extensions, testEntry, extensionRoot }),
      {
        env: {
          ...runtimeState.environment,
          RUNTROL_TEST_CORE: core,
          RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
          RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]),
          RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
          RUNTROL_VSCODE_ROLE: role,
          RUNTROL_VSCODE_COORDINATION: coordination,
        },
        stdio: "ignore",
        windowsHide: true,
      },
    );
    windows.push({ role, child, userData, workspace });
  }
  const ready = {};
  for (const { role } of windows) ready[role] = await waitForPublished(`${role}-ready.json`, 120_000);

  const workspaceLower = (folder) => folder.replace(/\\/g, "/").toLowerCase();
  const registry = async () => {
    const listed = probeJson(["windows-list", runtrolHome, identity, generationDigest]);
    return listed.windows;
  };
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, windows[0].workspace]).generation;
  const findWindow = (list, role) => list.find((window) => window.windowSessionId === ready[role].sessionId) ?? null;
  const steps = [];
  const record = async (label) => {
    const list = await registry();
    steps.push({ label, windows: list.map((window) => ({
      windowSessionId: window.windowSessionId.slice(0, 8),
      hostGeneration: window.hostGeneration,
      registrationGeneration: window.registrationGeneration,
      folders: window.workspaceFolders,
      terminals: window.terminals.map((terminal) => ({
        key: terminal.terminalKey, name: terminal.name, processId: terminal.processId ?? null,
        shellIntegration: terminal.shellIntegration, command: terminal.command?.commandLine ?? null,
      })),
    })) });
    return list;
  };

  await delay(1_000);
  const registered = await record("both windows registered");
  const bothRegistered = roles.every((role) => findWindow(registered, role) !== null)
    && roles.every((role) => {
      const window = findWindow(registered, role);
      const expected = windows.find((entry) => entry.role === role).workspace;
      return window.workspaceFolders.length === 1 && workspaceLower(window.workspaceFolders[0]) === workspaceLower(expected);
    });

  for (const { role } of windows) await publish(`${role}-open.json`, {});
  const opened = {};
  for (const { role } of windows) opened[role] = await waitForPublished(`${role}-opened.json`, 60_000);
  await delay(1_500);
  const afterOpen = await record("two terminals opened in each window");
  const terminalsFollowOpen = roles.every((role) => {
    const window = findWindow(afterOpen, role);
    const pids = window.terminals.map((terminal) => terminal.processId);
    return pids.includes(opened[role].one) && pids.includes(opened[role].two);
  });

  await publish("alpha-command.json", { run: true });
  await publish("beta-command.json", { run: false });
  const commanded = await waitForPublished("alpha-commanded.json", 60_000);
  await delay(800);
  const afterCommand = await record("a command runs in alpha's first terminal");
  const commandObserved = commanded.commandLine === "echo registry-command-one";

  for (const { role } of windows) await publish(`${role}-close.json`, {});
  for (const { role } of windows) await waitForPublished(`${role}-closed.json`, 60_000);
  await delay(1_000);
  const afterClose = await record("the second terminal closed in each window");
  const terminalsFollowClose = roles.every((role) => {
    const window = findWindow(afterClose, role);
    const pids = window.terminals.map((terminal) => terminal.processId);
    return pids.includes(opened[role].one) && !pids.includes(opened[role].two);
  });

  await publish("alpha-finish.json", { restart: false });
  await publish("beta-finish.json", { restart: false });
  // A window in extension-test mode cannot restart its host (measured 2026-09-02: the command is a no-op there,
  // and a finished test entry closes the window). The restart is measured on a third window that runs Studio the
  // way a person does: development mode, activated by Studio's own events, driven by keys from outside.
  const gamma = await developmentWindow("gamma");
  windows.push(gamma);
  const gammaTitle = "winreg-gamma";
  if (!(await waitForTitle(gammaTitle, 90_000, gamma.userData))) throw new Error("the development window never appeared by title");
  await delay(8_000);
  // Studio activates on its own view or commands, as it does for a person: one palette command wakes it.
  press(gammaTitle, "^+p", gamma.userData);
  await delay(1_500);
  press(gammaTitle, "Runtrol: Refresh Conversations{ENTER}", gamma.userData);
  const gammaRegistered = await waitForRegistry((list) => list.some((window) => sameFolder(window, gamma.workspace)), 120_000);
  const gammaBefore = gammaRegistered.find((window) => sameFolder(window, gamma.workspace));
  press(gammaTitle, "^+p", gamma.userData);
  await delay(1_500);
  press(gammaTitle, "Terminal: Create New Terminal{ENTER}", gamma.userData);
  let gammaWithTerminal;
  try {
    gammaWithTerminal = await waitForRegistry((list) => (list.find((window) => sameFolder(window, gamma.workspace))?.terminals ?? []).some((terminal) => terminal.processId), 60_000);
  } catch (error) {
    // The window's own words about the failure are in its notifications; keep a picture of them.
    const shot = path.join(executionRoot, "runtrol-winreg-eye");
    await mkdir(shot, { recursive: true });
    capture(gammaTitle, path.join(shot, "gammaFailure.png"), gamma.userData);
    throw error;
  }
  const gammaTerminal = gammaWithTerminal.find((window) => sameFolder(window, gamma.workspace)).terminals[0];
  await record("gamma opened a terminal by keys");
  press(gammaTitle, "^+p", gamma.userData);
  await delay(1_500);
  press(gammaTitle, "Developer: Restart Extension Host{ENTER}", gamma.userData);
  const alphaBefore = gammaBefore;
  // The restarted host registers the same window again on a new connection.
  let alphaAfter = null;
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const list = await registry();
    const candidate = list.find((window) => window.windowSessionId === gammaBefore.windowSessionId);
    if (candidate && candidate.hostGeneration !== gammaBefore.hostGeneration && candidate.terminals.length > 0) {
      alphaAfter = candidate;
      break;
    }
    await delay(500);
  }
  const afterRestart = await record("gamma's Extension Host restarted");
  const restartKeepsOneEntry = afterRestart.filter((window) => window.windowSessionId === gammaBefore.windowSessionId).length === 1;
  const restartReregisters = alphaAfter !== null
    && alphaAfter.registrationGeneration > gammaBefore.registrationGeneration
    && alphaAfter.terminals.some((terminal) => terminal.processId === gammaTerminal.processId);

  process.stdout.write(`${MARKER}${JSON.stringify({
    bothRegistered,
    terminalsFollowOpen,
    commandObserved,
    terminalsFollowClose,
    restartKeepsOneEntry,
    restartReregisters,
    steps,
  })}\n`);
} finally {
  for (const { child, userData } of windows) {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await terminateExactProcesses(userData, null);
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}

async function developmentWindow(role) {
  const userData = path.join(temporary, `${role}-user`);
  const extensions = path.join(temporary, `${role}-extensions`);
  const workspace = path.join(temporary, `${role}-workspace`);
  await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": `winreg-${role}` }),
    "utf8",
  );
  const child = spawn(
    executable,
    [
      workspace,
      ...isolatedLaunchArguments,
      "--disable-extensions",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensions}`,
      "--no-sandbox", "--disable-gpu-sandbox", "--disable-updates", "--skip-welcome", "--skip-release-notes",
      "--disable-workspace-trust",
      `--extensionDevelopmentPath=${extensionRoot}`,
    ],
    {
      env: { ...runtimeState.environment, RUNTROL_TEST_CORE: core, RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]) },
      stdio: "ignore",
      windowsHide: false,
    },
  );
  return { role, child, userData, workspace };
}

function sameFolder(window, workspace) {
  const lower = (folder) => folder.replace(/\\/g, "/").toLowerCase();
  return window.workspaceFolders.some((folder) => lower(folder) === lower(workspace));
}

async function waitForRegistry(test, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  let last = [];
  while (Date.now() < deadline) {
    last = probeJson(["windows-list", runtrolHome, identity, generationDigest]).windows;
    if (test(last)) return last;
    await delay(500);
  }
  throw new Error(`the registry never showed what was waited for; last ${JSON.stringify(last).slice(0, 800)}`);
}

function capture(titleMatch, outPath, commandLineMatch) {
  const shot = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", titleMatch, "-OutPath", outPath, "-CommandLineMatch", commandLineMatch],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

async function waitForTitle(titleMatch, deadlineMs, commandLineMatch) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const found = spawnSync(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "find-window.ps1"), "-TitleMatch", titleMatch, "-CommandLineMatch", commandLineMatch],
      { encoding: "utf8", timeout: 15_000, windowsHide: true },
    );
    if (found.stdout.trim()) return true;
    await delay(1_000);
  }
  return false;
}

function press(titleMatch, keys, commandLineMatch) {
  const pressed = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", titleMatch, "-Keys", keys, "-CommandLineMatch", commandLineMatch],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  if (pressed.status !== 0) throw new Error(`typing ${JSON.stringify(keys)} into ${JSON.stringify(titleMatch)} failed`);
}

function probeJson(words) {
  const ran = spawnSync(probe, words, { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (ran.status !== 0) throw new Error(`handoverProbe ${words[0]} failed: ${ran.stderr}${ran.stdout}`);
  return JSON.parse(ran.stdout.trim().split("\n").pop());
}

async function waitForPublished(name, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    for (const role of ["alpha", "beta"]) {
      const failed = await tryReadPublished(`${role}-failure.json`);
      if (failed) throw new Error(`${role} failed: ${failed.failure}\n${failed.stack ?? ""}`);
    }
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms`);
}

async function tryReadPublished(name) {
  try {
    const value = JSON.parse(await readFile(path.join(coordination, name), "utf8"));
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) return null;
    throw error;
  }
}

async function publish(name, value) {
  await writeFile(path.join(coordination, name), JSON.stringify(value), "utf8");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
