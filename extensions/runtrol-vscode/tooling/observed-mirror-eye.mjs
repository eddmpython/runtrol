// The observed-mirror journey (`EXT-02`): two isolated real VS Code windows on one isolated Runtime. A window runs
// a provider in an ordinary terminal (the fixture TUI by absolute path, real Claude and real Codex by absolute
// path, and a provider by name so the transparent shim brokers it); Studio's registry mirrors each on its own; this
// harness attaches a viewer through the public wire, holds the viewer's exact live bytes against what the window
// fed, holds the window's first bytes against a direct capture of the same program, checks the shim case yields one
// row, and captures the windows. Prints one `RUNTROL_OBSERVED_MIRROR {json}` line.
//
// Usage: node tooling/observed-mirror-eye.mjs [--keep-shots] [--steps=fixture,claude,codex,shim]
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

const MARKER = "RUNTROL_OBSERVED_MIRROR ";
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

const realProgram = (name) => {
  const found = spawnSync(process.platform === "win32" ? "where.exe" : "which", [name], { encoding: "utf8", windowsHide: true });
  const candidates = found.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const chosen = process.platform === "win32" ? candidates.find((entry) => entry.toLowerCase().endsWith(".cmd")) ?? candidates[0] : candidates[0];
  if (!chosen) throw new Error(`${name} is not on this machine's search path`);
  return chosen;
};
const claude = realProgram("claude");
const codex = realProgram("codex");

const keepShots = process.argv.includes("--keep-shots");
const onlySteps = (process.argv.find((word) => word.startsWith("--steps=")) ?? "--steps=fixture,claude,codex,shim").slice("--steps=".length).split(",");
const temporary = await mkdtemp(path.join(executionRoot, "runtrol-mirror-"));
const shots = path.join(executionRoot, "runtrol-mirror-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "observedMirror.test.cjs");
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
    entryPoints: [path.join(extensionRoot, "src", "integration", "observedMirror.test.ts")],
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

  for (const role of ["alpha", "beta"]) {
    const userData = path.join(temporary, `${role}-user`);
    const extensions = path.join(temporary, `${role}-extensions`);
    const workspace = path.join(temporary, `${role}-workspace`);
    await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
    await writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": `mirror-${role}` }),
      "utf8",
    );
    const child = spawn(
      executable,
      // Visible windows: the journey captures them.
      isolatedExtensionTestArguments({ workspace, userData, extensions, testEntry, extensionRoot, visual: true }),
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
        // A hidden start hides the workbench window itself (measured 2026-09-02); the journey captures it.
        windowsHide: false,
      },
    );
    windows.push({ role, child, userData, workspace, title: `mirror-${role}`, runs: 0 });
  }
  const ready = {};
  for (const { role } of windows) ready[role] = await waitForPublished(`${role}-ready.json`, 120_000);
  // Both workspaces are approved roots, so the viewer may attach to either window's mirrors.
  const enrolled = probeJson(["enroll", runtrolHome, core, identity, ...windows.map((window) => window.workspace)]);
  generationDigest = enrolled.generation;
  process.stdout.write(`enrolled ${JSON.stringify(enrolled).slice(0, 600)}
`);
  // The inventory must know the providers before a command can be recognised; give discovery a moment.
  await delay(6_000);

  const quoted = (program) => `& '${program.replace(/'/g, "''")}'`;
  const steps = [];
  const runStep = async (window, label, commandLine, exitKeys, exitKeyGapMs, settleMs, options = {}) => {
    window.runs += 1;
    const index = window.runs;
    await publish(`${window.role}-run-${index}.json`, { label, commandLine, exitKeys, exitKeyGapMs });
    const opened = await waitForPublished(`${window.role}-mirror-${index}.json`, 90_000);
    const step = { label, role: window.role, commandLine, opened: { terminalId: opened.terminalId, refusal: opened.refusal, providerId: opened.providerId, terminalKey: opened.terminalKey } };
    if (opened.terminalId) {
      const listedAtOpen = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
      step.listedAtOpen = listedAtOpen.map((terminal) => ({ id: terminal.terminalId.slice(0, 8), providerId: terminal.providerId, origin: terminal.origin, owner: terminal.ownerWindowSessionId?.slice(0, 8) ?? null, key: terminal.ownerTerminalKey, workspace: terminal.workspace }));
      process.stdout.write(`step ${label}: opened ${JSON.stringify(step.opened)} listed ${JSON.stringify(step.listedAtOpen)}
`);
      // The viewer attaches while the provider is still starting; its live bytes must be a suffix of the feed.
      const viewer = probeAsync(["bytes", runtrolHome, identity, generationDigest, opened.terminalId, String(settleMs)]);
      await delay(Math.max(1_000, settleMs - 2_500));
      step.listedWhileRunning = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.map((terminal) => ({ id: terminal.terminalId.slice(0, 8), providerId: terminal.providerId, origin: terminal.origin }));
      if (options.shot) capture(window.title, window.userData, path.join(shots, options.shot));
      await publish(`${window.role}-exit-${index}.json`, {});
      const [live, ended] = await Promise.all([viewer, waitForPublished(`${window.role}-ended-${index}.json`, 120_000)]);
      step.viewer = { origin: live.origin, ownerWindowSessionId: live.ownerWindowSessionId?.slice(0, 8) ?? null, ownerTerminalKey: live.ownerTerminalKey, checkpointBytes: live.checkpointBytes, chunks: live.chunks, lagged: live.lagged, liveBytes: live.liveBytes, exited: live.exited };
      step.fed = { bytes: ended.bytes, chunks: ended.chunks, sha256: ended.sha256.slice(0, 16), ended: ended.ended, exitCode: ended.exitCode, shellExitCode: ended.shellExitCode, timedOut: ended.timedOut, openDelayMs: ended.openedAtMs === null ? null : ended.openedAtMs - ended.startedAtMs, firstChunkDelayMs: ended.firstChunkAtMs === null ? null : ended.firstChunkAtMs - ended.startedAtMs, headHex: ended.headHex.slice(0, 160) };
      const fedHex = ended.headHex;
      // The viewer reads for its settle period while the owner keeps feeding, so its bytes are one contiguous run
      // of the feed: exactly the feed from where it attached to where it stopped reading.
      const at = live.liveHex.length > 0 ? fedHex.indexOf(live.liveHex) : -1;
      step.viewerIsRunOfFeed = at >= 0;
      step.viewerMissedBytes = at >= 0 ? at / 2 : null;
      step.ownerMatches = live.origin === "ObservedMirror" && live.ownerWindowSessionId === ready[window.role].sessionId && live.ownerTerminalKey === opened.terminalKey;
      if (options.direct) {
        const direct = probeJson(["direct-program", window.workspace, "4000", "-", options.direct.program, ...options.direct.arguments]);
        step.direct = { bytes: direct.bytes, exited: direct.exited, headHex: (direct.headHex ?? "").slice(0, 64) };
        // The owner's stream begins with VS Code's own command-executed marker (OSC 633;C) before the program's
        // first byte; the direct capture has no shell in front of it.
        const marker = "1b5d3633333b4307";
        const programBytes = fedHex.startsWith(marker) ? fedHex.slice(marker.length) : fedHex;
        step.ownerStartsWithShellMarker = fedHex.startsWith(marker);
        step.firstBytesMatchDirect = direct.headHex ? programBytes.startsWith(direct.headHex.slice(0, 64)) : null;
      }
      step.listedAfterEnd = await listedCount(10_000);
    } else {
      step.listedAtOpen = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.map((terminal) => ({ id: terminal.terminalId.slice(0, 8), providerId: terminal.providerId, origin: terminal.origin }));
      if (options.shot) capture(window.title, window.userData, path.join(shots, options.shot));
      await publish(`${window.role}-exit-${index}.json`, {});
      const ended = await waitForPublished(`${window.role}-ended-${index}.json`, 120_000);
      step.fed = { refusal: ended.refusal, ended: ended.ended, shellExitCode: ended.shellExitCode, timedOut: ended.timedOut };
      step.listedAfterEnd = await listedCount(10_000);
    }
    steps.push(step);
    process.stdout.write(`step ${label}: ${JSON.stringify(step).slice(0, 1500)}
`);
    return step;
  };

  // How many terminals the Runtime lists once the mirrors of an ended command have gone (0 when all ended).
  const listedCount = async (deadlineMs) => {
    const deadline = Date.now() + deadlineMs;
    let count = -1;
    while (Date.now() < deadline) {
      count = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.length;
      if (count === 0) return 0;
      await delay(500);
    }
    return count;
  };
  const [alpha, beta] = windows;
  const skipped = { opened: { terminalId: null, refusal: "skipped" }, listedAtOpen: [], listedAfterEnd: 0 };
  const maybe = (name, run) => (onlySteps.includes(name) ? run() : Promise.resolve(skipped));
  const fixtureStep = await maybe("fixture", () => runStep(alpha, "fixture", `${quoted(fixture)} --tui`, ["\u001a\r"], 2_500, 5_000, {
    direct: { program: fixture, arguments: ["--tui"] },
    shot: "alphaFixtureMirror.png",
  }));
  // A fresh folder asks Claude Code for trust first (Enter accepts); two Ctrl+C a second apart exit; the slash
  // command is the fallback.
  const claudeStep = await maybe("claude", () => runStep(alpha, "claude", quoted(claude), ["\r", "\u0003", "\u0003", "/exit\r"], 1_200, 9_000, {
    shot: "alphaClaudeMirror.png",
  }));
  const codexStep = await maybe("codex", () => runStep(beta, "codex", quoted(codex), ["\u0003", "\u0003", "/quit\r"], 2_500, 9_000, {
    shot: "betaCodexMirror.png",
  }));
  // By name: the transparent shim on the terminal's PATH brokers the command, so the Runtime keeps one row.
  const shimStep = await maybe("shim", () => runStep(alpha, "shim", "claude", ["\r", "\u0003", "\u0003", "/exit\r"], 1_200, 9_000, {
    shot: "alphaClaudeShim.png",
  }));

  // The shim's own terminal is the one row: either the mirror open was refused as brokered, or the mirror that
  // opened first was retired by the brokered open, leaving one Claude row while the command ran.
  const claudeRows = (listed) => listed.filter((terminal) => terminal.providerId === "claude");
  const shimOneRow = shimStep.opened.refusal !== null
    ? /broker/.test(shimStep.opened.refusal) && claudeRows(shimStep.listedAtOpen).length === 1
    : shimStep.listedWhileRunning !== undefined && claudeRows(shimStep.listedWhileRunning).length === 1
      && shimStep.listedWhileRunning.every((terminal) => terminal.providerId !== "claude" || terminal.origin === "Owned");

  process.stdout.write(`${MARKER}${JSON.stringify({
    // A viewer that attached after the fixture's whole output (a few dozen bytes) sees it in the checkpoint only.
    fixtureMirrored: fixtureStep.opened.terminalId !== null && fixtureStep.ownerMatches === true && (fixtureStep.viewerIsRunOfFeed === true || (fixtureStep.viewer?.liveBytes === 0 && fixtureStep.viewer?.checkpointBytes > 0)),
    claudeMirrored: claudeStep.opened.terminalId !== null && claudeStep.ownerMatches === true && claudeStep.viewerIsRunOfFeed === true,
    codexMirrored: codexStep.opened.terminalId !== null && codexStep.ownerMatches === true && codexStep.viewerIsRunOfFeed === true,
    shimOneRow,
    allEnded: steps.every((step) => step.listedAfterEnd === 0),
    steps,
  })}\n`);
} catch (error) {
  process.stdout.write(`daemon stderr tail:
${daemonStderr.slice(-4_000)}
`);
  throw error;
} finally {
  for (const window of windows) {
    try { await publish(`${window.role}-run-${window.runs + 1}.json`, { done: true }); } catch { /* the window may be gone */ }
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

// The window is found by the title its settings gave it, within its own process family (its user-data directory).
function capture(titleMatch, userData, outPath) {
  const found = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "find-window.ps1"), "-TitleMatch", titleMatch, "-CommandLineMatch", userData],
    { encoding: "utf8", timeout: 15_000, windowsHide: true },
  );
  const title = found.stdout.trim().split(/\r?\n/).pop() ?? "";
  if (!title) {
    // Say what is there instead, so a title or family mismatch is a fact rather than a guess.
    const titles = spawnSync(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-Command", "Get-Process Code | Where-Object { $_.MainWindowTitle } | ForEach-Object { \"$($_.Id) $($_.MainWindowTitle)\" }"],
      { encoding: "utf8", timeout: 15_000, windowsHide: true },
    );
    process.stdout.write(`capture: no window in family ${userData}; visible Code windows: ${titles.stdout.trim().replace(/\r?\n/g, " | ")}\n`);
    return;
  }
  const shot = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", title, "-OutPath", outPath, "-CommandLineMatch", userData],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

function probeJson(words) {
  const ran = spawnSync(probe, words, { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (ran.status !== 0) throw new Error(`handoverProbe ${words[0]} failed: ${ran.stderr}${ran.stdout}`);
  return JSON.parse(ran.stdout.trim().split("\n").pop());
}

function probeAsync(words) {
  return new Promise((resolve, reject) => {
    const child = spawn(probe, words, { env: daemonEnvironment, windowsHide: true });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => { out += chunk; });
    child.stderr.on("data", (chunk) => { err += chunk; });
    child.on("exit", (code) => {
      if (code !== 0) reject(new Error(`handoverProbe ${words[0]} failed: ${err}${out}`));
      else resolve(JSON.parse(out.trim().split("\n").pop()));
    });
  });
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

