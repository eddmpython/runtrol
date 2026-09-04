// The owner-focus journey (`EXT-05`): a provider running in a VS Code terminal with shell integration turned OFF.
//
// Two isolated real VS Code windows on one isolated Runtime. Alpha has shell integration disabled and types a real
// provider into an ordinary terminal, so nothing can be mirrored from it. Beta, which has both folders as projects,
// finds that conversation's row and clicks it. The row must say it can only be shown where it runs, the click must
// bring alpha forward showing that exact terminal, and no mirror may exist for it anywhere.
// Prints one `RUNTROL_FOCUS_OWNER {json}` line.
//
// Usage: node tooling/focus-owner-eye.mjs [--keep-shots] [--provider=claude|codex]
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  acquireVSCode,
  isolatedExtensionTestArguments,
  isolatedProfileSettings,
  isolatedRuntimeState,
  terminateExactProcesses,
} from "./isolated-vscode.mjs";

const MARKER = "RUNTROL_FOCUS_OWNER ";
const providerName = (process.argv.find((word) => word.startsWith("--provider=")) ?? "--provider=claude").slice("--provider=".length);
const keepShots = process.argv.includes("--keep-shots");
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

const found = spawnSync(process.platform === "win32" ? "where.exe" : "which", [providerName], { encoding: "utf8", windowsHide: true });
const candidates = found.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
const program = process.platform === "win32" ? candidates.find((entry) => entry.toLowerCase().endsWith(".cmd")) ?? candidates[0] : candidates[0];
if (!program) throw new Error(`${providerName} is not on this machine's search path`);

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-focus-"));
const shots = path.join(executionRoot, "runtrol-focus-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };

let daemon = null;
let failed = false;
const windows = [];
let executable = null;
let generationDigest = null;
try {
  await Promise.all([coordination, runtrolHome, shots].map((d) => mkdir(d, { recursive: true })));
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "ownerReveal.test.ts")],
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

  const projectFolders = ["alpha", "beta"].map((role) => path.join(temporary, `${role}-project`));
  for (const role of ["alpha", "beta"]) {
    const userData = path.join(temporary, `${role}-user`);
    const extensions = path.join(temporary, `${role}-extensions`);
    const workspace = path.join(temporary, `${role}-project`);
    await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
    await writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({
        ...isolatedProfileSettings,
        "runtrol.corePath": core,
        "window.title": `focus-${role}`,
        // Alpha is the window under test: no shell integration, so no mirror is possible there.
        ...(role === "alpha" ? { "terminal.integrated.shellIntegration.enabled": false } : {}),
      }),
      "utf8",
    );
    const child = spawn(
      executable,
      isolatedExtensionTestArguments({ workspace, userData, extensions, testEntry, extensionRoot, visual: true }),
      {
        env: {
          ...runtimeState.environment,
          RUNTROL_TEST_CORE: core,
          RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
          RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify(projectFolders),
          RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
          RUNTROL_VSCODE_ROLE: role,
          RUNTROL_VSCODE_COORDINATION: coordination,
        },
        stdio: "ignore",
        windowsHide: false,
      },
    );
    windows.push({ role, child, userData, workspace, title: `focus-${role}`, steps: 0 });
  }
  const ready = {};
  for (const { role } of windows) ready[role] = await waitForPublished(`${role}-ready.json`, 120_000);
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...projectFolders]).generation;
  await delay(6_000);

  const [alpha, beta] = windows;
  const step = async (window, body) => {
    window.steps += 1;
    await publish(`${window.role}-step-${window.steps}.json`, body);
    return waitForPublished(`${window.role}-done-${window.steps}.json`, 180_000);
  };
  for (const window of windows) {
    for (const folder of projectFolders) await step(window, { kind: "addProject", folder });
  }
  // What the roster already knows, so the conversation alpha is about to start is told apart from the operator's own.
  const before = new Set(probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]).live ?? []);
  const started = await step(alpha, {
    kind: "startTyped",
    label: "provider",
    commandLine: `& '${program.replace(/'/g, "''")}'`,
    // Measured 2026-09-02 from the window itself: the folder-trust question defaults to "No, exit", so a bare
    // Enter closes the provider. The answer is one arrow down, then Enter. A row is a conversation, and the
    // provider writes one only once something is said in it (measured 2026-09-04: a provider sitting at its prompt
    // is in the process roster and proved focusable, and on no row anywhere), so one short message follows.
    setupKeys: ["\u001b[B", "\r", "Reply with the single word ok.", "\r"],
    setupGapMs: 6_000,
    settleMs: 20_000,
  });
  // What the provider is actually showing after its first-run questions: a screen nobody looked at is how a
  // journey ends up asserting against a guess.
  capture(alpha.title, alpha.userData, path.join(shots, "alphaAfterStart.png"));
  // Beta is looking at something else of its own, so a focus has to move the desktop, not just repaint alpha.
  await step(beta, { kind: "showOther" });

  // The Runtime must prove the conversation is live and that a window owns its terminal before any row can say so.
  const roster = await waitForFocusable(providerName, before, 90_000);
  const fresh = (roster.live ?? []).filter((native) => !before.has(native));
  const nativeId = (roster.focusable ?? []).find((native) => fresh.includes(native)) ?? null;
  const rowKey = nativeId === null ? null : `chat:${encodeURIComponent(providerName)}:${encodeURIComponent(nativeId)}`;
  const terminals = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;

  let clicked = null;
  let alphaAfter = null;
  let foregroundBefore = null;
  let foregroundAfter = null;
  let terminalsAfterClick = [];
  if (rowKey !== null) {
    activate(beta.title, beta.userData);
    await delay(800);
    capture(beta.title, beta.userData, path.join(shots, "betaBeforeClick.png"));
    foregroundBefore = foreground();
    clicked = await step(beta, { kind: "click", key: rowKey });
    await delay(1_500);
    foregroundAfter = foreground();
    alphaAfter = await step(alpha, { kind: "report" });
    capture(alpha.title, alpha.userData, path.join(shots, "alphaAfterFocus.png"));
    // What the click did in the clicking window: nothing may have opened there, and the Runtime may hold no
    // terminal for this conversation afterwards either (no console mirror, no attachment).
    capture(beta.title, beta.userData, path.join(shots, "betaAfterClick.png"));
    terminalsAfterClick = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
  }

  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    program,
    mirrorsOpened: started.mirrorsOpenedAfterwards,
    shellIntegration: started.shellIntegration,
    shellProcessId: started.shellProcessId,
    roster,
    freshLive: (roster.live ?? []).filter((native) => !before.has(native)),
    nativeId,
    rowKey,
    mirroredTerminals: terminals.map((terminal) => ({ providerId: terminal.providerId, origin: terminal.origin })),
    terminalsAfterClick: terminalsAfterClick.map((terminal) => ({ providerId: terminal.providerId, origin: terminal.origin })),
    clicked,
    rowFacts: clicked?.facts ?? null,
    rowWaitMs: clicked?.rowWaitMs ?? null,
    // The words on the row follow these two facts: `Focus owner` is canFocus without canOpen.
    saidFocusOwner: clicked?.facts?.canFocus === true && clicked?.facts?.canOpen === false,
    foregroundBefore,
    foregroundAfter,
    alphaActiveTerminal: alphaAfter?.activeTerminalName ?? null,
    // The three answers the stamp is about.
    neverMirrored: started.mirrorsOpenedAfterwards === 0 && terminals.length === 0 && terminalsAfterClick.length === 0,
    windowProved: nativeId !== null,
    shownWhereItRuns: clicked?.reveal?.delivered === true
      && alphaAfter?.activeTerminalName === "alpha-provider"
      && String(foregroundAfter ?? "").includes(alpha.title),
  })}\n`);
} catch (error) {
  // A run that ended badly keeps its evidence: the coordination files, the isolated home and the daemon it used.
  failed = true;
  process.stdout.write(`kept for inspection: ${temporary}
`);
  throw error;
} finally {
  for (const window of windows) {
    try { await publish(`${window.role}-step-${window.steps + 1}.json`, { kind: "done" }); } catch { /* the window may be gone */ }
  }
  for (const { child, userData } of windows) {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await terminateExactProcesses(userData, null);
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  if (!failed) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

// The roster answers only after the provider has minted its conversation identity, which takes a moment.
async function waitForFocusable(provider, before, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  let last = null;
  while (Date.now() < deadline) {
    last = probeJson(["native-activity", runtrolHome, identity, generationDigest, provider]);
    const fresh = (last.live ?? []).filter((native) => !before.has(native));
    process.stdout.write(`roster: fresh ${JSON.stringify(fresh)} focusable ${JSON.stringify(last.focusable ?? [])}
`);
    if ((last.focusable ?? []).some((native) => fresh.includes(native))) return last;
    await delay(3_000);
  }
  return last ?? { live: [], attachable: [], focusable: [], active: [] };
}

// How many processes of one isolated profile are still running: zero means the window is gone, not merely stuck.
function profileProcesses(userData) {
  const ran = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", `@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${userData.replace(/'/g, "''")}*' }).Count`],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  return Number(ran.stdout.trim()) || 0;
}

function foreground() {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "foreground-window.ps1")], { encoding: "utf8", timeout: 15_000, windowsHide: true });
  return ran.stdout.trim();
}

function activate(titleMatch, userData) {
  const pressed = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", titleMatch, "-Keys", "{F16}", "-CommandLineMatch", userData],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
}

function capture(titleMatch, userData, outPath) {
  const shot = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", titleMatch, "-OutPath", outPath, "-CommandLineMatch", userData],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
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
      if (failed) throw new Error(`${role} failed: ${failed.failure}`);
    }
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  const alive = {};
  for (const role of ["alpha", "beta"]) alive[role] = await tryReadPublished(`${role}-alive.json`);
  let listing = [];
  try { listing = await readdir(coordination); } catch (error) { listing = [`unreadable: ${String(error).slice(0, 120)}`]; }
  const profiles = {};
  for (const window of windows) profiles[window.role] = profileProcesses(window.userData);
  throw new Error(`${name} did not arrive within ${deadlineMs} ms; heartbeats ${JSON.stringify(alive)}; coordination ${JSON.stringify(listing)}; processes ${JSON.stringify(profiles)}`);
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
