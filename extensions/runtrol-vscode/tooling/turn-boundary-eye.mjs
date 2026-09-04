// The turn boundary journey (`STATE-02`): the running icon follows the provider's own structural turn state and
// nothing else.
//
// One isolated real VS Code window on one isolated Runtime, one project folder, one provider started from the
// sidebar's own `+`. The sidebar row's activity is sampled every few hundred milliseconds and held, sample by
// sample, against the provider's own record of whether a model is answering (for Claude Code the `status` of its
// `sessions/<pid>.json` roster record, which the isolated driver reads too). Four phases: idle after a first
// answer (never working), a turn whose tool call keeps the screen quiet for a while (working for as long as the
// provider says so), the turn's end (working stops within a bounded lag), and a redraw of the prompt while idle
// (output flows on a public view and the row still never reads as working). Prints one
// `RUNTROL_TURN_BOUNDARY {json}` line.
//
// Usage: node tooling/turn-boundary-eye.mjs [--keep-shots] [--provider=claude]
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, open, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
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

const MARKER = "RUNTROL_TURN_BOUNDARY ";
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
const found = spawnSync("where.exe", [providerName], { encoding: "utf8", windowsHide: true });
if (!found.stdout.trim()) throw new Error(`${providerName} is not on this machine's search path (where.exe status ${found.status})`);

// The provider's own turn state, read the way the isolated driver reads it and nowhere else: a structural field
// of the provider's own record, never the screen. Claude Code writes `<config>/sessions/<pid>.json` with the
// conversation id and a `status` of `busy`, `idle` or `waiting`; `busy` is a model answering.
const claudeConfig = process.env.CLAUDE_CONFIG_DIR
  ? path.resolve(process.env.CLAUDE_CONFIG_DIR)
  : path.join(os.homedir(), ".claude");
async function providerTurnState(native) {
  if (providerName !== "claude") return { status: null, why: `no turn-state reader for ${providerName}` };
  const directory = path.join(claudeConfig, "sessions");
  let names = [];
  try { names = await readdir(directory); } catch (error) { return { status: null, why: `no roster: ${String(error).slice(0, 80)}` }; }
  for (const name of names) {
    if (!name.endsWith(".json")) continue;
    let record;
    try { record = JSON.parse(await readFile(path.join(directory, name), "utf8")); } catch { continue; }
    if (record && record.sessionId === native) return { status: record.status ?? null, pid: record.pid ?? null, file: name };
  }
  return { status: null, why: "no record names this conversation" };
}

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-turn-"));
const shots = path.join(executionRoot, "runtrol-turn-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const project = path.join(temporary, "turn-project");
const userData = path.join(temporary, "viewer-user");
const extensions = path.join(temporary, "viewer-extensions");
const workspace = path.join(temporary, "viewer-project");

let daemon = null;
let daemonLog = null;
let failed = false;
let viewer = null;
let executable = null;
let generationDigest = null;
const samples = [];
try {
  await Promise.all([coordination, runtrolHome, shots, project, path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
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
  daemonLog = await open(path.join(temporary, "daemon.log"), "w");
  daemon = spawn(core, ["daemon"], { env: daemonEnvironment, stdio: ["ignore", daemonLog.fd, daemonLog.fd], windowsHide: true });
  await delay(500);
  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": "turn-viewer" }),
    "utf8",
  );
  const roots = [workspace, project];
  const child = spawn(
    executable,
    isolatedExtensionTestArguments({ workspace, userData, extensions, testEntry, extensionRoot, visual: true }),
    {
      env: {
        ...runtimeState.environment,
        RUNTROL_TEST_CORE: core,
        RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
        RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify(roots),
        RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
        RUNTROL_VSCODE_ROLE: "viewer",
        RUNTROL_VSCODE_COORDINATION: coordination,
      },
      stdio: "ignore",
      windowsHide: false,
    },
  );
  viewer = { child, userData, title: "turn-viewer", steps: 0 };
  await waitForPublished("viewer-ready.json", 120_000);
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  await delay(4_000);
  await step({ kind: "addProject", folder: project });

  // One provider from the sidebar's own `+`: folder trust, then one short answer so the conversation is named.
  await step({ kind: "startFresh", provider: providerName, workspace: project });
  await waitFor("the placeholder and its running terminal", 30_000, async () => {
    const rows = await projectRows();
    const owned = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
    return rows.some((row) => row.key.startsWith("started:")) && owned.some((t) => t.origin === "Owned" && t.processState === "Running");
  });
  const placeholderKey = (await projectRows()).find((row) => row.key.startsWith("started:"))?.key ?? null;
  if (!placeholderKey) throw new Error("no placeholder row for the + path");
  activate();
  await delay(500);
  await step({ kind: "click", key: placeholderKey });
  await delay(1_500);
  pressViewer("{DOWN}{ENTER}");
  await delay(6_000);
  pressViewer("Reply with the single word ok.{ENTER}");
  const named = await waitFor("the named hosted conversation", 90_000, async () => (await projectRows()).find((row) => row.key.startsWith("chat:") && row.presence === "hosted") ?? null);
  const native = named.native;
  const terminalId = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.find((t) => t.nativeSessionId === native)?.terminalId ?? null;
  if (!terminalId) throw new Error("the named conversation has no Runtime terminal");

  // Sampling: the row's activity beside the provider's own turn state, every few hundred milliseconds.
  let phase = "settle";
  let sampling = true;
  const sampler = (async () => {
    while (sampling) {
      const rows = await projectRows();
      const row = rows.find((candidate) => candidate.native === native) ?? null;
      const truth = await providerTurnState(native);
      samples.push({
        atMs: Date.now(),
        phase,
        activity: row?.activity ?? null,
        working: row?.activity === "working",
        status: truth.status,
        busy: truth.status === "busy",
      });
      await delay(350);
    }
  })();

  // Phase A: idle after the first answer. The provider says idle and the row never works.
  await waitFor("the first answer to end", 60_000, async () => (await providerTurnState(native)).status === "idle");
  await delay(1_000);
  phase = "idle";
  await delay(5_000);
  activate();
  await delay(300);
  capture(path.join(shots, "idle.png"));

  // Phase B: a turn whose tool call keeps the model busy for a while with little on the screen. The provider's
  // own status says busy for its whole length; the row works for exactly that long and nothing else decides it.
  phase = "turn";
  const turnAskedAt = Date.now();
  pressViewer("Use the Bash tool to run this exact command and then reply with the single word done: powershell -NoProfile -Command Start-Sleep -Seconds 15{ENTER}");
  // Output on the public view during the middle of the turn: how much the screen moves while the tool sleeps.
  await delay(5_000);
  const midTurnBytes = probeJson(["bytes", runtrolHome, identity, generationDigest, terminalId, "4000"]);
  await delay(500);
  capture(path.join(shots, "midTurn.png"));
  await waitFor("the turn to end", 120_000, async () => (await providerTurnState(native)).status === "idle");
  const turnEndedAt = Date.now();
  phase = "after-turn";
  await delay(4_000);
  capture(path.join(shots, "afterTurn.png"));

  // Phase D: a redraw of the prompt while idle. A public view counts the output while the screen is cleared and
  // the editor's sidebar is toggled twice (a resize makes the provider draw its whole screen again); the row
  // never reads as working.
  phase = "redraw";
  activate();
  await delay(300);
  const redrawBytesPromise = probeJsonAsync(["bytes", runtrolHome, identity, generationDigest, terminalId, "6000"]);
  await delay(800);
  pressViewer("^l");
  await delay(700);
  pressViewer("^b");
  await delay(900);
  pressViewer("^b");
  await delay(700);
  pressViewer("^l");
  const redrawBytes = await redrawBytesPromise;
  await delay(1_500);
  capture(path.join(shots, "redraw.png"));
  sampling = false;
  await sampler;

  // The exit: the provider's own command, so nothing is left behind.
  await step({ kind: "click", key: named.key });
  await delay(600);
  pressViewer("/exit{ENTER}");
  await waitFor("the terminal to end", 45_000, async () => !probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals.some((t) => t.terminalId === terminalId));

  // Judgement. Every sample either agrees with the provider or sits inside the bounded lag after a transition.
  const LAG_MS = 2_000;
  const transitions = [];
  for (let index = 1; index < samples.length; index += 1) {
    if (samples[index].busy !== samples[index - 1].busy) transitions.push({ atMs: samples[index].atMs, busy: samples[index].busy });
  }
  const withinLag = (sample) => transitions.some((t) => sample.atMs >= t.atMs && sample.atMs - t.atMs <= LAG_MS);
  const disagreements = samples.filter((sample) => sample.status !== null && sample.working !== sample.busy && !withinLag(sample));
  const idleSamples = samples.filter((sample) => sample.phase === "idle");
  const turnSamples = samples.filter((sample) => sample.phase === "turn");
  const redrawSamples = samples.filter((sample) => sample.phase === "redraw");
  const afterSamples = samples.filter((sample) => sample.phase === "after-turn");
  const workingLagMs = (() => {
    const busyAt = samples.find((sample) => sample.busy)?.atMs ?? null;
    const workingAt = samples.find((sample) => sample.working && busyAt !== null && sample.atMs >= busyAt)?.atMs ?? null;
    return busyAt !== null && workingAt !== null ? workingAt - busyAt : null;
  })();
  const stoppedLagMs = (() => {
    const lastBusy = [...samples].reverse().find((sample) => sample.busy)?.atMs ?? null;
    const stoppedAt = samples.find((sample) => lastBusy !== null && sample.atMs > lastBusy && !sample.working)?.atMs ?? null;
    return lastBusy !== null && stoppedAt !== null ? stoppedAt - lastBusy : null;
  })();
  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    native,
    terminalId,
    samples: samples.length,
    idleNeverWorking: idleSamples.length > 0 && idleSamples.every((sample) => !sample.working && sample.status === "idle"),
    turnLengthMs: turnEndedAt - turnAskedAt,
    turnBusySamples: turnSamples.filter((sample) => sample.busy).length,
    turnWorkingWhileBusy: turnSamples.filter((sample) => sample.busy).length > 0 && turnSamples.filter((sample) => sample.busy && !sample.working && !withinLag(sample)).length === 0,
    midTurnOutput: { chunks: midTurnBytes.chunks, lagged: midTurnBytes.lagged, checkpointBytes: midTurnBytes.checkpointBytes, exited: midTurnBytes.exited ?? null },
    afterTurnNeverWorking: afterSamples.length > 0 && afterSamples.every((sample) => !sample.working || withinLag(sample)),
    redrawOutput: { chunks: redrawBytes.chunks, lagged: redrawBytes.lagged },
    redrawOutputFlowed: redrawBytes.chunks > 0,
    redrawNeverWorking: redrawSamples.length > 0 && redrawSamples.every((sample) => !sample.working && sample.status === "idle"),
    workingLagMs,
    stoppedLagMs,
    disagreements,
    followsProviderTurnState: disagreements.length === 0,
    timeline: samples.map((sample) => `${sample.phase}@${sample.atMs % 1000000}:${sample.status ?? "-"}/${sample.activity ?? "-"}`),
  })}\n`);
} catch (error) {
  failed = true;
  process.stdout.write(`kept for inspection: ${temporary}\n`);
  throw error;
} finally {
  if (viewer) {
    try { await publish(`viewer-step-${viewer.steps + 1}.json`, { kind: "done" }); } catch { /* the window may be gone */ }
    if (viewer.child.exitCode === null && viewer.child.signalCode === null) viewer.child.kill("SIGKILL");
    await terminateExactProcesses(viewer.userData, null);
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  if (daemonLog) {
    await daemonLog.close();
    try {
      const said = await readFile(path.join(temporary, "daemon.log"), "utf8");
      process.stdout.write(`daemon said:\n${said.split(/\r?\n/).filter(Boolean).slice(-20).join("\n")}\n`);
    } catch { /* nothing was said */ }
  }
  if (!failed) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

async function projectRows() {
  const answer = await step({ kind: "rows" });
  return answer.rows.filter((row) => row.workspace.toLowerCase() === project.toLowerCase());
}

async function waitFor(what, deadlineMs, condition) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const value = await condition();
    if (value) return value;
    await delay(400);
  }
  throw new Error(`${what} did not happen within ${deadlineMs} ms`);
}

async function step(body) {
  viewer.steps += 1;
  await publish(`viewer-step-${viewer.steps}.json`, body);
  return waitForPublished(`viewer-done-${viewer.steps}.json`, 180_000);
}

function pressKeys(titleMatch, userData, keys) {
  const pressed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", titleMatch, "-Keys", keys, "-CommandLineMatch", userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  return pressed.status === 0;
}

function pressViewer(keys) {
  return pressKeys(viewer.title, viewer.userData, keys);
}

function activate() {
  pressViewer("{F16}");
}

function capture(outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", viewer.title, "-OutPath", outPath, "-CommandLineMatch", viewer.userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

// The same probe, started without blocking this process, for a window that must span keys pressed meanwhile.
function probeJsonAsync(words) {
  return new Promise((resolve, reject) => {
    const child = spawn(probe, words, { env: daemonEnvironment, windowsHide: true });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => { out += chunk; });
    child.stderr.on("data", (chunk) => { err += chunk; });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code !== 0) { reject(new Error(`handoverProbe ${words[0]} failed: ${err}${out}`)); return; }
      try { resolve(JSON.parse(out.trim().split("\n").pop())); } catch (error) { reject(error); }
    });
  });
}

function probeJson(words) {
  const ran = spawnSync(probe, words, { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (ran.status !== 0) throw new Error(`handoverProbe ${words[0]} failed: ${ran.stderr}${ran.stdout}`);
  return JSON.parse(ran.stdout.trim().split("\n").pop());
}

async function waitForPublished(name, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const failure = await tryReadPublished("viewer-failure.json");
    if (failure) throw new Error(`viewer failed: ${failure.failure}`);
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  const alive = await tryReadPublished("viewer-alive.json");
  let listing = [];
  try { listing = await readdir(coordination); } catch (error) { listing = [`unreadable: ${String(error).slice(0, 120)}`]; }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms; heartbeat ${JSON.stringify(alive)}; coordination ${JSON.stringify(listing)}`);
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
