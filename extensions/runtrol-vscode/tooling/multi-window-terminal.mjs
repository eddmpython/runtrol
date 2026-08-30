import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  isolatedExtensionTestArguments,
  isolatedProfileSettings,
  terminateExactProcesses,
  withoutHostIdentity,
} from "./isolated-vscode.mjs";

const core = requiredEnvironment("RUNTROL_TEST_CORE");
const vscode = requiredEnvironment("RUNTROL_TEST_VSCODE_EXECUTABLE");
const workspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE");
const workRoot = requiredEnvironment("RUNTROL_VSCODE_WORK_ROOT");
const coordination = path.join(workRoot, "coordination");
const output = path.join(workRoot, "tests");
const testEntry = path.join(output, "multiWindowTerminal.test.cjs");
const ownerUserData = path.join(workRoot, "owner-user");
const mirrorUserData = path.join(workRoot, "mirror-user");
const ownerExtensions = path.join(workRoot, "owner-extensions");
const mirrorExtensions = path.join(workRoot, "mirror-extensions");
const ownerPidPath = requiredEnvironment("RUNTROL_ACP_FIXTURE_TUI_PID_PATH");
const testEnvironment = withoutHostIdentity();
let owner = null;
let mirror = null;

try {
  await Promise.all([
    stat(core),
    stat(vscode),
    stat(workspace),
    mkdir(output, { recursive: true }),
    mkdir(coordination, { recursive: true }),
    writeProfile(ownerUserData),
    writeProfile(mirrorUserData),
    mkdir(ownerExtensions, { recursive: true }),
    mkdir(mirrorExtensions, { recursive: true }),
  ]);
  const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
    cwd: extensionRoot,
    env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
    encoding: "utf8",
    windowsHide: true,
  });
  if (bundled.status !== 0) {
    throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);
  }
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "multiWindowTerminal.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });

  owner = launch("owner", ownerUserData, ownerExtensions);
  const ownerReady = await waitForPublished("owner-ready.json", 60_000);
  const ownerPid = await readOwnerPid(ownerPidPath);
  const ownerAliveBeforeMirror = processAlive(ownerPid);
  if (!ownerAliveBeforeMirror) throw new Error(`terminal owner PID ${ownerPid} exited before the mirror opened`);

  mirror = launch("mirror", mirrorUserData, mirrorExtensions);
  const mirrorArmed = await waitForPublished("mirror-armed.json", 60_000);
  const ownerAliveWhileBothOpen = processAlive(ownerPid);
  await waitForPublished("owner-result.json", 60_000);
  await requireCleanExit(owner, "owner VS Code", 20_000);
  await terminateExactProcesses(ownerUserData, null);
  const ownerAliveAfterOwnerWindowClosed = processAlive(ownerPid);
  await publish("owner-closed.json", { ownerPid });

  const mirrorResult = await waitForPublished("mirror-result.json", 60_000);
  await requireCleanExit(mirror, "mirror VS Code", 20_000);
  await terminateExactProcesses(mirrorUserData, null);
  const providerStopped = await waitForProcessExit(ownerPid, 10_000);
  const ownerResult = await readPublished("owner-result.json");
  const sameTerminal = sameTerminalIdentity(ownerReady, mirrorArmed)
    && sameTerminalIdentity(ownerReady, ownerResult.terminal)
    && sameTerminalIdentity(ownerReady, mirrorResult.terminal);
  process.stdout.write(`RUNTROL_VSCODE_MULTI_WINDOW ${JSON.stringify({
    sameTerminal,
    terminalId: ownerReady.terminalId,
    runtimeGeneration: ownerReady.runtimeGeneration,
    ownerPid,
    oneOwnerPid: Number.isSafeInteger(ownerPid) && ownerPid > 0,
    ownerAliveBeforeMirror,
    ownerAliveWhileBothOpen,
    ownerAliveAfterOwnerWindowClosed,
    ownerSawOwnerInput: Number.isFinite(ownerResult.ownerInputMs),
    mirrorSawOwnerInput: Number.isFinite(mirrorResult.mirrorSawOwnerMs),
    mirrorWroteAfterOwnerWindowClosed: Number.isFinite(mirrorResult.mirrorInputAfterHandoffMs),
    mirrorSawOwnInput: Number.isFinite(mirrorResult.mirrorInputAfterHandoffMs),
    ownerInputMs: ownerResult.ownerInputMs,
    mirrorSawOwnerMs: mirrorResult.mirrorSawOwnerMs,
    mirrorInputAfterHandoffMs: mirrorResult.mirrorInputAfterHandoffMs,
    providerStopped,
    ownerVscode: ownerResult.vscode,
    mirrorVscode: mirrorResult.vscode,
  })}\n`);
} finally {
  const cleanupErrors = [];
  for (const profile of [ownerUserData, mirrorUserData]) {
    try {
      await terminateExactProcesses(profile, null);
    } catch (error) {
      cleanupErrors.push(error);
    }
  }
  for (const child of [owner, mirror]) {
    if (child && child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  }
  await rm(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  if (cleanupErrors.length > 0) {
    throw new AggregateError(cleanupErrors, "multi-window VS Code cleanup failed");
  }
}

function launch(role, userData, extensions) {
  const arguments_ = isolatedExtensionTestArguments({
    workspace,
    userData,
    extensions,
    testEntry,
    extensionRoot,
  });
  return spawn(vscode, arguments_, {
    env: {
      ...testEnvironment,
      RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
      RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
      RUNTROL_VSCODE_ROLE: role,
      RUNTROL_VSCODE_COORDINATION: coordination,
    },
    stdio: "inherit",
    windowsHide: true,
  });
}

async function writeProfile(userData) {
  const directory = path.join(userData, "User");
  await mkdir(directory, { recursive: true });
  await writeFile(
    path.join(directory, "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "runtrol.followWorkspace": true,
    }),
    "utf8",
  );
}

async function waitForPublished(name, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    for (const role of ["owner", "mirror"]) {
      const failed = await tryReadPublished(`${role}-failure.json`);
      if (failed) throw new Error(`${role} VS Code failed: ${failed.failure}\n${failed.stack ?? ""}`);
    }
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms`);
}

async function readPublished(name) {
  const value = await tryReadPublished(name);
  if (!value) throw new Error(`${name} is absent`);
  return value;
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

function sameTerminalIdentity(left, right) {
  return left?.runtimeGeneration === right?.runtimeGeneration
    && left?.terminalId === right?.terminalId
    && left?.terminalGeneration === right?.terminalGeneration;
}

async function readOwnerPid(file) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    try {
      const raw = (await readFile(file, "utf8")).trim();
      if (/^[1-9][0-9]*$/.test(raw)) return Number(raw);
      throw new Error(`terminal owner marker is invalid: ${JSON.stringify(raw)}`);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    await delay(25);
  }
  throw new Error("the terminal owner process wrote no PID marker");
}

function processAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return process.platform === "win32" && error.code === "EPERM";
  }
}

async function waitForProcessExit(pid, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    if (!processAlive(pid)) return true;
    await delay(25);
  }
  return !processAlive(pid);
}

async function requireCleanExit(child, label, deadlineMs) {
  const result = await new Promise((resolve, reject) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve({ code: child.exitCode, signal: child.signalCode });
      return;
    }
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`${label} did not exit within ${deadlineMs} ms`));
    }, deadlineMs);
    const exited = (code, signal) => {
      cleanup();
      resolve({ code, signal });
    };
    const failed = (error) => {
      cleanup();
      reject(error);
    };
    const cleanup = () => {
      clearTimeout(timer);
      child.off("exit", exited);
      child.off("error", failed);
    };
    child.once("exit", exited);
    child.once("error", failed);
  });
  if (result.code !== 0 || result.signal !== null) {
    throw new Error(`${label} exited as ${String(result.code ?? result.signal)}`);
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
