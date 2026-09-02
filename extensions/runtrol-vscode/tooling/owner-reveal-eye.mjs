// The owner-reveal journey (`EXT-03`): two isolated real VS Code windows on different projects, one isolated
// Runtime. Each window files both folders as projects, starts a real provider in an ordinary terminal (an observed
// mirror), and then clicks the other window's terminal row. The harness reads from the desktop which window holds
// the foreground afterwards, from each window which terminal it shows as active, and captures both windows.
// Prints one `RUNTROL_OWNER_REVEAL {json}` line.
//
// Usage: node tooling/owner-reveal-eye.mjs [--keep-shots] [--fixture]
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
  isolatedProfileSettings,
  isolatedRuntimeState,
  terminateExactProcesses,
} from "./isolated-vscode.mjs";

const MARKER = "RUNTROL_OWNER_REVEAL ";
const FIXTURE_PROVIDER = "fixture-acp";
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const executionRoot = process.platform === "win32"
  ? path.join(requiredEnvironment("LOCALAPPDATA"), "dev-workspace")
  : path.join(os.homedir(), ".local", "share", "dev-workspace");
await mkdir(executionRoot, { recursive: true });
const suffix = process.platform === "win32" ? ".exe" : "";
const target = path.join(repositoryRoot, "target", "debug");
const core = path.join(target, `runtrol${suffix}`);
const probe = path.join(target, "examples", `handoverProbe${suffix}`);
const fixture = path.join(target, "examples", `acpFixture${suffix}`);
for (const [packageName, kind, name] of [
  ["runtrol", "--bin", "runtrol"],
  ["runtrol", "--example", "handoverProbe"],
  ["runtrol-drivers", "--example", "acpFixture"],
]) {
  const built = spawnSync("cargo", ["build", "-p", packageName, kind, name], { cwd: repositoryRoot, encoding: "utf8", windowsHide: true });
  if (built.status !== 0) throw new Error(`cargo build ${name} failed:\n${built.stderr}`);
}
await Promise.all([stat(core), stat(probe), stat(fixture)]);
const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);

const useFixture = process.argv.includes("--fixture");
const realProgram = (name) => {
  const found = spawnSync(process.platform === "win32" ? "where.exe" : "which", [name], { encoding: "utf8", windowsHide: true });
  const candidates = found.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const chosen = process.platform === "win32" ? candidates.find((entry) => entry.toLowerCase().endsWith(".cmd")) ?? candidates[0] : candidates[0];
  if (!chosen) throw new Error(`${name} is not on this machine's search path`);
  return chosen;
};
const quoted = (program) => `& '${program.replace(/'/g, "''")}'`;
const programs = useFixture
  ? { alpha: `${quoted(fixture)} --tui`, beta: `${quoted(fixture)} --tui`, exit: ["\u001a\r"] }
  : { alpha: quoted(realProgram("claude")), beta: quoted(realProgram("codex")), exit: ["\r", "\u0003", "\u0003", "/exit\r", "/quit\r"] };

const keepShots = process.argv.includes("--keep-shots");
const temporary = await mkdtemp(path.join(executionRoot, "runtrol-reveal-"));
const shots = path.join(executionRoot, "runtrol-reveal-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment, PATH: `${path.dirname(fixture)}${path.delimiter}${process.env.PATH ?? ""}` };

let daemon = null;
let daemonStderr = "";
const windows = [];
let executable = null;
let generationDigest = null;
try {
  await Promise.all([coordination, runtrolHome, shots, path.join(runtrolHome, "providers")].map((d) => mkdir(d, { recursive: true })));
  await writeFile(
    path.join(runtrolHome, "providers", `${FIXTURE_PROVIDER}.toml`),
    [
      "schema = 1",
      `id = "${FIXTURE_PROVIDER}"`,
      'display_name = "Terminal Fixture"',
      'kind = "acp"',
      "",
      "[bin]",
      `names = [${JSON.stringify(path.basename(fixture))}]`,
      "",
      "[probe]",
      'version = { args = ["--version"], parse = "semver-anywhere" }',
      "",
      "[transport]",
      "argv = []",
      'listen = "stdio"',
      "",
      "[tui]",
      'new = ["--tui"]',
      "",
    ].join("\n"),
    "utf8",
  );
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
  daemon = spawn(core, ["daemon"], { env: daemonEnvironment, stdio: ["ignore", "ignore", "pipe"], windowsHide: true });
  daemon.stderr.on("data", (chunk) => { daemonStderr += chunk; if (daemonStderr.length > 200_000) daemonStderr = daemonStderr.slice(-100_000); });
  await delay(500);
  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));

  // Each window's approved roots are both projects: the person filed both folders, and a filed project is an
  // approved root, which is what lets one window list the other's terminal at all.
  const projectFolders = ["alpha", "beta"].map((role) => path.join(temporary, `${role}-project`));
  for (const role of ["alpha", "beta"]) {
    const userData = path.join(temporary, `${role}-user`);
    const extensions = path.join(temporary, `${role}-extensions`);
    const workspace = path.join(temporary, `${role}-project`);
    await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
    await writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": `reveal-${role}` }),
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
        // A hidden start hides the workbench window itself (measured 2026-09-02); the journey captures it.
        windowsHide: false,
      },
    );
    windows.push({ role, child, userData, workspace, title: `reveal-${role}`, steps: 0 });
  }
  const ready = {};
  for (const { role } of windows) ready[role] = await waitForPublished(`${role}-ready.json`, 120_000);
  const enrolled = probeJson(["enroll", runtrolHome, core, identity, ...windows.map((window) => window.workspace)]);
  generationDigest = enrolled.generation;
  await delay(6_000);

  const [alpha, beta] = windows;
  const step = async (window, body) => {
    window.steps += 1;
    await publish(`${window.role}-step-${window.steps}.json`, body);
    return waitForPublished(`${window.role}-done-${window.steps}.json`, 120_000);
  };
  const report = {};
  // Both folders are projects in both windows: the other window's terminal row is then listed here.
  for (const window of windows) {
    await step(window, { kind: "addProject", folder: alpha.workspace });
    await step(window, { kind: "addProject", folder: beta.workspace });
  }
  const started = {
    alpha: await step(alpha, { kind: "start", label: "provider", commandLine: programs.alpha }),
    beta: await step(beta, { kind: "start", label: "provider", commandLine: programs.beta }),
  };
  report.started = started;
  await delay(4_000);
  const listed = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
  report.listed = listed.map((terminal) => ({ id: terminal.terminalId.slice(0, 8), providerId: terminal.providerId, origin: terminal.origin, owner: terminal.ownerWindowSessionId?.slice(0, 8) ?? null, key: terminal.ownerTerminalKey, workspace: path.basename(terminal.workspace) }));
  const rowKey = (terminalId) => `terminal:${encodeURIComponent(generationDigest)}:${encodeURIComponent(terminalId)}`;
  const alphaRow = rowKey(started.alpha.terminalId);
  const betaRow = rowKey(started.beta.terminalId);
  const before = { alpha: await step(alpha, { kind: "report" }), beta: await step(beta, { kind: "report" }) };
  report.rowsBefore = { alphaSeesBeta: before.alpha.rowKeys.includes(betaRow), betaSeesAlpha: before.beta.rowKeys.includes(alphaRow), alphaRows: before.alpha.rowKeys.length, betaRows: before.beta.rowKeys.length };
  process.stdout.write(`rows ${JSON.stringify({ listed: report.listed, before, alphaRow, betaRow })}
`);

  const foreground = () => {
    const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "foreground-window.ps1")], { encoding: "utf8", timeout: 15_000, windowsHide: true });
    return ran.stdout.trim();
  };
  const clicks = [];
  for (const [clicker, owner, key] of [[beta, alpha, alphaRow], [alpha, beta, betaRow]]) {
    // The clicker holds the foreground first, as a person's window does when they click in it.
    // The owner is looking at another terminal, and the clicker holds the foreground as a person's window does
    // when they click in it.
    const ownerBefore = await step(owner, { kind: "showOther" });
    activate(clicker.title, clicker.userData);
    await delay(800);
    capture(clicker.title, clicker.userData, path.join(shots, `${clicker.role}BeforeClick.png`));
    const foregroundBefore = foreground();
    const clicked = await step(clicker, { kind: "click", key });
    process.stdout.write(`click ${clicker.role} -> ${owner.role}: ${JSON.stringify(clicked)}\n`);
    await delay(1_500);
    const foregroundAfter = foreground();
    const ownerReport = await step(owner, { kind: "report" });
    capture(owner.title, owner.userData, path.join(shots, `${owner.role}AfterReveal.png`));
    clicks.push({
      clicker: clicker.role,
      owner: owner.role,
      clickedMs: clicked.clickedMs,
      reveal: clicked.reveal,
      ownerActiveBefore: ownerBefore.activeTerminalName,
      foregroundBefore,
      foregroundAfter,
      ownerActiveTerminal: ownerReport.activeTerminalName,
      ownerHoldsForeground: foregroundAfter.includes(owner.title),
      ownerShowsTerminal: ownerReport.activeTerminalName === `${owner.role}-provider`,
    });
  }
  report.clicks = clicks;

  for (const window of windows) await step(window, { kind: "exit", label: "provider", keys: programs.exit, gapMs: 1_200 });
  await delay(3_000);
  report.listedAfterExit = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.length;

  process.stdout.write(`${MARKER}${JSON.stringify({
    bothListEachOther: report.rowsBefore.alphaSeesBeta && report.rowsBefore.betaSeesAlpha,
    ownerShowsTerminal: clicks.every((click) => click.ownerShowsTerminal),
    ownerComesForward: clicks.every((click) => click.ownerHoldsForeground),
    ...report,
  })}\n`);
} catch (error) {
  process.stdout.write(`daemon stderr tail:\n${daemonStderr.slice(-4_000)}\n`);
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
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

// Give a window the foreground the way a person's click in it does: the key-pressing tool activates it and types
// F16, a key VS Code binds to nothing.
function activate(titleMatch, userData) {
  const pressed = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", titleMatch, "-Keys", "{F16}", "-CommandLineMatch", userData],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
}

// The window is found by the title its settings gave it, within its own process family (its user-data directory).
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
      if (failed) throw new Error(`${role} failed: ${failed.failure}\n${failed.stack ?? ""}`);
    }
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  // Which window stopped, and whether it is frozen or waiting: each window beats once a second with the step it is on.
  const alive = {};
  for (const role of ["alpha", "beta"]) alive[role] = await tryReadPublished(`${role}-alive.json`);
  for (const window of windows) capture(window.title, window.userData, path.join(shots, `${window.role}Timeout.png`));
  throw new Error(`${name} did not arrive within ${deadlineMs} ms; heartbeats ${JSON.stringify(alive)}`);
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
