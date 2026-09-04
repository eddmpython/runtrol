// The lifecycle record journey (`STATE-03`): open tab, closed tab, owner focus, mirror availability and Stop state
// all read the same generation record, from both windows, through every lifecycle path.
//
// Two isolated real VS Code windows (alpha, beta) on one isolated Runtime, one project filed in both. A provider
// starts from alpha's own `+`; beta opens the same conversation from its row; both close their tabs and the process
// keeps its record; beta reopens; alpha stops it from the row while both windows are sampled through the stop; beta
// reopens the stored conversation (a new record) and alpha stops that one too; finally a provider started by
// absolute path in an ordinary alpha terminal shows in beta as an observed mirror naming alpha as its owner, and
// ending it ends the record. Prints one `RUNTROL_LIFECYCLE_RECORD {json}` line.
//
// Usage: node tooling/lifecycle-record-eye.mjs [--keep-shots] [--provider=claude]
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

const MARKER = "RUNTROL_LIFECYCLE_RECORD ";
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
const candidates = found.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
// The absolute program is what an ordinary terminal runs when the transparent shim is not in the way: an observed
// mirror rather than a brokered terminal (`EXT-02`).
const program = candidates.find((entry) => entry.toLowerCase().endsWith(".cmd")) ?? candidates[0];
if (!program) throw new Error(`${providerName} is not on this machine's search path`);

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-record-"));
const shots = path.join(executionRoot, "runtrol-record-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const project = path.join(temporary, "record-project");

let daemon = null;
let daemonLog = null;
let failed = false;
let executable = null;
let generationDigest = null;
const windows = [];
const timeline = [];
try {
  await Promise.all([coordination, runtrolHome, shots, project].map((d) => mkdir(d, { recursive: true })));
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

  const roots = [];
  for (const role of ["alpha", "beta"]) {
    const userData = path.join(temporary, `${role}-user`);
    const extensions = path.join(temporary, `${role}-extensions`);
    const workspace = path.join(temporary, `${role}-workspace`);
    await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
    await writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": `record-${role}` }),
      "utf8",
    );
    roots.push(workspace);
    windows.push({ role, userData, extensions, workspace, title: `record-${role}`, steps: 0, child: null, sessionId: null });
  }
  roots.push(project);
  for (const window of windows) {
    window.child = spawn(
      executable,
      isolatedExtensionTestArguments({ workspace: window.workspace, userData: window.userData, extensions: window.extensions, testEntry, extensionRoot, visual: true }),
      {
        env: {
          ...runtimeState.environment,
          RUNTROL_TEST_CORE: core,
          RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
          RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify(roots),
          RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
          RUNTROL_VSCODE_ROLE: window.role,
          RUNTROL_VSCODE_COORDINATION: coordination,
        },
        stdio: "ignore",
        windowsHide: false,
      },
    );
  }
  for (const window of windows) {
    const ready = await waitForPublished(`${window.role}-ready.json`, 120_000);
    window.sessionId = ready.sessionId ?? null;
  }
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  await delay(6_000);
  const [alpha, beta] = windows;
  for (const window of windows) await step(window, { kind: "addProject", folder: project });

  // One conversation from alpha's own `+`, named after one short answer. Beta lists it from the same record.
  await step(alpha, { kind: "startFresh", provider: providerName, workspace: project });
  await waitFor("alpha's placeholder and its running terminal", 30_000, async () => {
    const rows = await projectRows(alpha);
    const owned = terminals().filter((t) => t.origin === "Owned" && t.processState === "Running");
    return rows.some((row) => row.key.startsWith("started:")) && owned.length > 0;
  });
  const placeholderKey = (await projectRows(alpha)).find((row) => row.key.startsWith("started:"))?.key ?? null;
  if (!placeholderKey) throw new Error("no placeholder row in alpha");
  activate(alpha);
  await delay(500);
  await step(alpha, { kind: "click", key: placeholderKey });
  await delay(1_500);
  press(alpha, "{DOWN}{ENTER}");
  await delay(6_000);
  press(alpha, "Reply with the single word ok.{ENTER}");
  const named = await waitFor("the named hosted row in both windows", 90_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a && b && a.key === b.key ? { key: a.key, alpha: a, beta: b } : null;
  });
  const key = named.key;
  snapshot("named", named.alpha, named.beta);

  // Beta opens the same conversation from its row: the same record, now open in both windows.
  await step(beta, { kind: "click", key });
  const bothOpen = await waitFor("the tab to be open in both windows", 30_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a?.open && b?.open ? { alpha: a, beta: b } : null;
  });
  snapshot("both-open", bothOpen.alpha, bothOpen.beta);
  for (const window of windows) { activate(window); await delay(300); capture(window, path.join(shots, `bothOpen-${window.role}.png`)); }

  // Beta closes its tab, then alpha closes its own: the process and its record stay, nobody has a tab.
  await step(beta, { kind: "closeTab", key });
  const betaClosed = await waitFor("beta's tab to close", 20_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a && b && !b.open && a.open ? { alpha: a, beta: b } : null;
  });
  snapshot("beta-closed", betaClosed.alpha, betaClosed.beta);
  await step(alpha, { kind: "closeTab", key });
  const bothClosed = await waitFor("alpha's tab to close with the record kept", 20_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a && b && !a.open && !b.open ? { alpha: a, beta: b } : null;
  });
  snapshot("both-closed", bothClosed.alpha, bothClosed.beta);

  // Beta reopens the same record; alpha stops it from the row. Both windows are sampled through the stop.
  await step(beta, { kind: "click", key });
  const reopened = await waitFor("beta to reopen the same record", 30_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return b?.open && a && !a.open && b.hostedKey === bothClosed.beta.hostedKey ? { alpha: a, beta: b } : null;
  });
  snapshot("beta-reopened", reopened.alpha, reopened.beta);
  const firstStop = await stopAndSample(alpha, key, "first-stop");
  const stopped = await waitFor("both windows to show the stored conversation", 30_000, async () => {
    const [a, b] = await Promise.all([rowByKey(alpha, key), rowByKey(beta, key)]);
    return a && b && a.presence === "stored" && b.presence === "stored" && !a.open && !b.open ? { alpha: a, beta: b } : null;
  });
  snapshot("stopped", stopped.alpha, stopped.beta);

  // Beta reopens the stored conversation: a new record, hosted for both, open only in beta. Alpha stops it.
  await step(beta, { kind: "click", key });
  const resumed = await waitFor("the resumed conversation as a new record in both windows", 90_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a && b && b.open && !a.open && a.hostedKey === b.hostedKey && a.hostedKey !== firstStop.hostedKey ? { alpha: a, beta: b } : null;
  });
  snapshot("resumed", resumed.alpha, resumed.beta);
  await delay(4_000);
  for (const window of windows) { activate(window); await delay(300); capture(window, path.join(shots, `resumed-${window.role}.png`)); }
  const secondStop = await stopAndSample(alpha, key, "second-stop");
  const stoppedAgain = await waitFor("both windows to show the stored conversation again", 30_000, async () => {
    const [a, b] = await Promise.all([rowByKey(alpha, key), rowByKey(beta, key)]);
    return a && b && a.presence === "stored" && b.presence === "stored" ? { alpha: a, beta: b } : null;
  });
  snapshot("stopped-again", stoppedAgain.alpha, stoppedAgain.beta);

  // Owner and mirror: a provider started by absolute path in an ordinary alpha terminal is an observed mirror whose
  // record names alpha as the owner window; ending it ends the record everywhere.
  const mirrorStart = await step(alpha, { kind: "start", label: "mirror", commandLine: `& '${program.replace(/'/g, "''")}'`, cwd: project });
  await step(alpha, { kind: "type", label: "mirror", keys: ["\u001b[B", "\r"], gapMs: 1_500 });
  await delay(6_000);
  await step(alpha, { kind: "type", label: "mirror", keys: ["Reply with the single word ok.", "\r"], gapMs: 800 });
  const mirrorRow = await waitFor("beta to list the mirror with its owner", 90_000, async () => {
    const rows = await projectRows(beta);
    return rows.find((row) => row.origin === "observedMirror" && row.key.startsWith("chat:")) ?? null;
  });
  const mirrorInAlpha = (await projectRows(alpha)).find((row) => row.key === mirrorRow.key) ?? null;
  // One row for the conversation and its mirror: no bare terminal row is left beside the named one.
  const bareMirrorRows = (await projectRows(beta)).filter((row) => row.key.startsWith("terminal:") && row.origin === "observedMirror");
  snapshot("mirror", mirrorInAlpha, mirrorRow);
  for (const window of windows) { activate(window); await delay(300); capture(window, path.join(shots, `mirror-${window.role}.png`)); }
  await step(alpha, { kind: "exit", label: "mirror", keys: ["/exit", "\r"], gapMs: 800 });
  const mirrorEnded = await waitFor("the mirror's record to end in both windows", 45_000, async () => {
    const [a, b] = await Promise.all([rowByKey(alpha, mirrorRow.key), rowByKey(beta, mirrorRow.key)]);
    const gone = (row) => row === null || row.hostedKey === null;
    return gone(a) && gone(b) ? { alpha: a, beta: b } : null;
  });
  snapshot("mirror-ended", mirrorEnded.alpha, mirrorEnded.beta);

  const sameRecord = (entry) => entry.alpha?.hostedKey && entry.alpha.hostedKey === entry.beta?.hostedKey;
  const stopJudged = (stop) => ({
    samples: stop.samples.length,
    stoppingSeen: stop.samples.some((s) => s.alpha?.stopping || s.beta?.stopping),
    everSaidElsewhere: stop.samples.some((s) => [s.alpha, s.beta].some((row) => row && (row.presence === "external" || row.presence === "unconfirmed"))),
    everStoppableDuringStop: stop.samples.some((s) => [s.alpha, s.beta].some((row) => row?.stopping && row.canStop)),
    recordKeptWhileStopping: stop.samples.filter((s) => s.alpha?.stopping).every((s) => s.alpha.hostedKey === stop.hostedKey),
    betaTabClosedByExit: stop.samples.length > 0 && (stop.samples[stop.samples.length - 1].beta?.open === false),
    durationMs: stop.durationMs,
  });
  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    key,
    sameRecordAtStart: sameRecord(named),
    sameRecordWhenBothOpen: sameRecord(bothOpen) && bothOpen.alpha.open && bothOpen.beta.open,
    closeKeepsRecord: sameRecord(bothClosed) && bothClosed.alpha.presence === "hosted" && bothClosed.beta.presence === "hosted" && bothClosed.alpha.hostedKey === named.alpha.hostedKey,
    reopenSameRecord: reopened.beta.hostedKey === named.alpha.hostedKey && reopened.beta.open,
    firstStop: stopJudged(firstStop),
    stoppedStoredInBoth: stopped.alpha.presence === "stored" && stopped.beta.presence === "stored",
    resumeIsNewRecord: resumed.alpha.hostedKey !== firstStop.hostedKey && sameRecord(resumed) && resumed.beta.open && !resumed.alpha.open,
    secondStop: stopJudged(secondStop),
    mirror: { origin: mirrorRow.origin, ownerWindow: mirrorRow.ownerWindow, alphaSession: alpha.sessionId, canFocusInBeta: mirrorRow.canFocus, alphaSeesOwned: mirrorInAlpha?.origin ?? null, terminalId: mirrorStart.terminalId ?? null },
    mirrorRecordNamesOwner: mirrorRow.origin === "observedMirror" && mirrorRow.ownerWindow === alpha.sessionId,
    mirrorIsOneRow: bareMirrorRows.length === 0 && mirrorRow.presence === "hosted" && mirrorRow.canStop === false,
    bareMirrorRows: bareMirrorRows.map((row) => row.key),
    mirrorEndsRecordEverywhere: (mirrorEnded.alpha === null || mirrorEnded.alpha.hostedKey === null) && (mirrorEnded.beta === null || mirrorEnded.beta.hostedKey === null),
    timeline,
  })}\n`);
} catch (error) {
  failed = true;
  process.stdout.write(`kept for inspection: ${temporary}\n`);
  throw error;
} finally {
  for (const window of windows) {
    if (!window.child) continue;
    try { await publish(`${window.role}-step-${window.steps + 1}.json`, { kind: "done" }); } catch { /* the window may be gone */ }
    if (window.child.exitCode === null && window.child.signalCode === null) window.child.kill("SIGKILL");
    await terminateExactProcesses(window.userData, null);
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

// Stop from one window's row and sample both windows until the terminal is gone from the Runtime.
async function stopAndSample(window, key, label) {
  const before = await rowByKey(window, key);
  const hostedKey = before?.hostedKey ?? null;
  const terminalId = hostedKey ? hostedKey.split(":").pop() : null;
  const started = Date.now();
  await step(window, { kind: "stopRow", key });
  const samples = [];
  let photographed = false;
  while (Date.now() - started < 20_000) {
    const [a, b] = await Promise.all([rowByKey(windows[0], key), rowByKey(windows[1], key)]);
    samples.push({ atMs: Date.now() - started, alpha: pick(a), beta: pick(b) });
    // The stop is short (measured 2026-09-05: under a second), so the first sample that says stopping is
    // photographed at once, in both windows, before the process is gone.
    if (!photographed && (a?.stopping || b?.stopping)) {
      photographed = true;
      for (const each of windows) capture(each, path.join(shots, `${label}-${each.role}.png`));
    }
    const stillListed = terminals().some((t) => t.terminalId === terminalId);
    if (!stillListed && a?.presence !== "hosted" && b?.presence !== "hosted") break;
    await delay(200);
  }
  timeline.push({ label, samples });
  return { hostedKey, samples, durationMs: Date.now() - started };
}

function pick(row) {
  return row ? { presence: row.presence, hostedKey: row.hostedKey, open: row.open, live: row.live, canStop: row.canStop, canOpen: row.canOpen, canFocus: row.canFocus, stopping: row.stopping, origin: row.origin, ownerWindow: row.ownerWindow } : null;
}

function snapshot(label, alphaRow, betaRow) {
  timeline.push({ label, alpha: pick(alphaRow), beta: pick(betaRow) });
}

function terminals() {
  return probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
}

async function projectRows(window) {
  const answer = await step(window, { kind: "rows" });
  return answer.rows.filter((row) => row.workspace.toLowerCase() === project.toLowerCase());
}

async function hostedRow(window) {
  return (await projectRows(window)).find((row) => row.key.startsWith("chat:") && row.presence === "hosted") ?? null;
}

async function rowByKey(window, key) {
  return (await projectRows(window)).find((row) => row.key === key) ?? null;
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

async function step(window, body) {
  window.steps += 1;
  await publish(`${window.role}-step-${window.steps}.json`, body);
  return waitForPublished(`${window.role}-done-${window.steps}.json`, 180_000);
}

function press(window, keys) {
  const pressed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", window.title, "-Keys", keys, "-CommandLineMatch", window.userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  return pressed.status === 0;
}

function activate(window) {
  press(window, "{F16}");
}

function capture(window, outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", window.title, "-OutPath", outPath, "-CommandLineMatch", window.userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
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
      const failure = await tryReadPublished(`${role}-failure.json`);
      if (failure) throw new Error(`${role} failed: ${failure.failure}`);
    }
    const value = await tryReadPublished(name);
    if (value) return value;
    await delay(25);
  }
  let listing = [];
  try { listing = await readdir(coordination); } catch (error) { listing = [`unreadable: ${String(error).slice(0, 120)}`]; }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms; coordination ${JSON.stringify(listing)}`);
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
