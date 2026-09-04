// The dead generation journey (`EXT-08`): dead process generations and cold provider records leave nothing behind.
//
// One isolated real VS Code window on one isolated Runtime. Three owners of one provider start: one through the
// sidebar's own `+` (a Runtrol-owned terminal), one typed by name into a VS Code terminal of the same project (the
// transparent shim brokers it into a second owned terminal), and one on the desktop in Windows Terminal (a focus
// owner the Runtime never hosts). Every owner is ended the way a person ends it, one exited conversation is
// reopened and ended again, and then the Runtime generation is killed outright (no withdraw: its locator entry
// stays behind, which is the dead generation), a new generation starts, and finally the window itself is
// restarted. The sidebar is read at each stage and judged: after the owners end nothing is live, nothing offers
// Stop or Focus, no row says `Elsewhere` (running but unavailable) or `Unavailable` (owner unconfirmed); after the
// Runtime returns the dead generation is gone from the locator and from the listing's warnings; after the window
// restarts the same holds and the stored conversations are still listed. Prints one
// `RUNTROL_DEAD_GENERATION {json}` line.
//
// Usage: node tooling/dead-generation-eye.mjs [--keep-shots] [--provider=claude]
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

const MARKER = "RUNTROL_DEAD_GENERATION ";
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
const program = candidates.find((entry) => entry.toLowerCase().endsWith(".cmd")) ?? candidates[0];
if (!program) throw new Error(`${providerName} is not on this machine's search path (where.exe status ${found.status}, said ${JSON.stringify(`${found.stdout}${found.stderr}`.slice(0, 200))})`);
const wt = spawnSync("where.exe", ["wt.exe"], { encoding: "utf8", windowsHide: true }).stdout.split(/\r?\n/).map((l) => l.trim()).find(Boolean);
if (!wt) throw new Error("Windows Terminal (wt.exe) is not installed on this machine");

// The two sentences a blocked row carries, exactly as `conversationList.ts` writes them. Either one after every
// owner has ended is the stale badge this journey exists to catch.
const RUNNING_ELSEWHERE = "This conversation is already running, but its live terminal is not available in this window.";
const PROCESS_STATUS_UNAVAILABLE = "Runtrol could not confirm whether this conversation is still running, so it will not open a second owner.";

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-deadgen-"));
const shots = path.join(executionRoot, "runtrol-deadgen-eye");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const project = path.join(temporary, "deadgen-project");
const hostFolder = path.join(temporary, "deadgen-desktop");
const stamp = `runtrol-ext08-${process.pid}`;
const host = { name: "wt", folder: hostFolder, title: `${stamp}-wt`, shell: null, owner: null, nativeId: null };
const userData = path.join(temporary, "viewer-user");
const extensions = path.join(temporary, "viewer-extensions");
const workspace = path.join(temporary, "viewer-project");
const roots = [workspace, project, hostFolder];

let daemon = null;
let daemonLog = null;
let failed = false;
let viewer = null;
let executable = null;
let generationDigest = null;
const ownedRoots = [];
const timeline = [];
try {
  await Promise.all([runtrolHome, shots, project, hostFolder, path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
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
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": "deadgen-viewer" }),
    "utf8",
  );
  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));

  daemon = await startDaemon("daemon-1.log");
  viewer = await launchViewer(1);
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  await delay(4_000);
  await step({ kind: "addProject", folder: project });
  await step({ kind: "addProject", folder: hostFolder });
  await snapshot("baseline");

  // Owner 1: the sidebar's own `+`, a Runtrol-owned terminal. Folder trust, then one message so it is named.
  // The click waits for the Runtime's own terminal to be running under the placeholder before it answers: the
  // folder-trust prompt is that terminal's, and answering before it is drawn sends `{DOWN}{ENTER}` at nothing, or
  // at the default `No, exit`, which ends the process (measured 2026-09-05: the plus terminal appeared and vanished).
  await step({ kind: "startFresh", provider: providerName, workspace: project });
  await snapshotFor("plus-starting", 30_000, (rows) => rows.some((row) => row.key.startsWith("started:"))
    && (timeline[timeline.length - 1]?.terminals ?? []).some((terminal) => terminal.origin === "Owned" && terminal.state === "Running"));
  const placeholderKey = latestRows().find((row) => row.key.startsWith("started:"))?.key ?? null;
  if (!placeholderKey) throw new Error("the + path never produced a placeholder row with a running terminal");
  activate();
  await delay(500);
  await step({ kind: "click", key: placeholderKey });
  await delay(1_500);
  pressViewer("{DOWN}{ENTER}");
  await delay(6_000);
  pressViewer("Reply with the single word ok.{ENTER}");
  await snapshotFor("plus-naming", 90_000, (rows) => rows.some((row) => row.key.startsWith("chat:") && row.presence === "hosted"));
  const plusRow = latestRows().find((row) => row.key.startsWith("chat:") && row.presence === "hosted") ?? null;
  if (!plusRow) throw new Error("the + owner never became a named hosted conversation (it likely exited at its folder-trust prompt)");

  // Owner 2: typed by name into an ordinary VS Code terminal of the same project; the shim brokers it. Its
  // questions are answered with keys into that terminal.
  const typed = await step({ kind: "start", label: "typed", commandLine: providerName, cwd: project });
  await snapshotFor("typed-starting", 20_000, (rows) => rows.filter((row) => row.presence === "hosted").length >= 2);
  await step({ kind: "type", label: "typed", keys: ["\u001b[B", "\r"], gapMs: 1_500 });
  await delay(6_000);
  await step({ kind: "type", label: "typed", keys: ["Reply with the single word ok.", "\r"], gapMs: 800 });
  await snapshotFor("typed-naming", 90_000, (rows) => rows.filter((row) => row.key.startsWith("chat:") && row.presence === "hosted").length >= 2);
  const typedRow = latestRows().find((row) => row.key.startsWith("chat:") && row.presence === "hosted" && row.key !== plusRow?.key) ?? null;

  // Owner 3: the provider on the desktop in Windows Terminal, in the second project folder. Never hosted, only
  // observed through the provider's own roster and focusable through the window its process owns.
  const before = new Set(probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]).live ?? []);
  {
    const processesBefore = processTable();
    // Not `windowsHide`, and no `;` in the command line (measured 2026-09-05, `external-terminal-eye.mjs`).
    spawn(wt, ["-w", "new", "new-tab", "--title", host.title, "--suppressApplicationTitle", "-d", host.folder, "powershell", "-NoExit", "-Command", `& '${program.replace(/'/g, "''")}'`], { detached: true, stdio: "ignore" }).unref();
    let processesAfter = processTable();
    let fresh = [];
    for (let waited = 0; waited < 25_000 && host.owner === null; waited += 1_000) {
      await delay(1_000);
      processesAfter = processTable();
      fresh = [...processesAfter.keys()].filter((pid) => !processesBefore.has(pid));
      host.shell = fresh.find((pid) => processesAfter.get(pid).name.toLowerCase() === "powershell.exe") ?? null;
      host.owner = windowsTitled(host.title)[0]?.pid ?? null;
    }
    ownedRoots.push(...fresh.filter((pid) => ["windowsterminal.exe", "powershell.exe", "conhost.exe", "openconsole.exe"].includes(processesAfter.get(pid).name.toLowerCase())));
    if (host.owner === null) throw new Error(`no Windows Terminal window appeared for ${host.title}`);
    pressKeys({ pid: host.owner }, "{DOWN}{ENTER}");
    await delay(6_000);
    pressKeys({ pid: host.owner }, "Reply with the single word ok.{ENTER}");
    const roster = await waitForFresh(before, 90_000);
    host.nativeId = (roster.live ?? []).find((native) => !before.has(native)) ?? null;
  }
  const desktopKey = host.nativeId ? `chat:${encodeURIComponent(providerName)}:${encodeURIComponent(host.nativeId)}` : null;
  await snapshotFor("desktop-listing", 60_000, (rows) => desktopKey !== null && rows.some((row) => row.key === desktopKey && row.presence === "external"));
  activate();
  await delay(500);
  capture(path.join(shots, "ownersLive.png"));
  const ownersLive = latestRows();
  const owners = {
    plus: plusRow ? facts(ownersLive, plusRow.key) : null,
    typed: typedRow ? facts(ownersLive, typedRow.key) : null,
    desktop: desktopKey ? facts(ownersLive, desktopKey) : null,
  };

  // End every owner the way a person does: the provider's own exit command in each.
  if (plusRow) {
    await step({ kind: "click", key: plusRow.key });
    await delay(800);
    pressViewer("/exit{ENTER}");
  }
  await step({ kind: "exit", label: "typed", keys: ["/exit", "\r"], gapMs: 800 });
  if (host.owner) {
    const typedExit = pressKeys({ pid: host.owner }, "/exit{ENTER}");
    if (!typedExit) {
      const table = processTable();
      for (const pid of descendantsOf(table, host.shell).filter((pid) => table.get(pid).name.toLowerCase().startsWith(providerName.toLowerCase()))) {
        spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", windowsHide: true });
      }
    }
  }
  const rosterGone = await waitForGone([plusRow?.native, typedRow?.native, host.nativeId].filter(Boolean), 60_000);
  if (host.owner) pressKeys({ pid: host.owner }, "exit{ENTER}");
  const ownersEnded = await snapshotFor("owners-ended", 45_000, (rows) => staleRows(rows).length === 0);
  await delay(3_000);
  const ownersEndedSettled = await snapshot("owners-ended-settled");
  capture(path.join(shots, "ownersEnded.png"));

  // One exited conversation reopens (the resume path) and is ended again: a cold record that was hot once more.
  const reopened = await step({ kind: "reopenStored", provider: providerName });
  const reopenedRows = await snapshotFor("reopened", 60_000, (rows) => rows.some((row) => row.presence === "hosted"));
  const reopenedRow = reopenedRows.find((row) => row.presence === "hosted") ?? null;
  if (reopenedRow) {
    await step({ kind: "click", key: reopenedRow.key });
    await delay(800);
    pressViewer("/exit{ENTER}");
  }
  await snapshotFor("reopened-ended", 45_000, (rows) => staleRows(rows).length === 0);
  await delay(3_000);
  const reopenedEnded = await snapshot("reopened-ended-settled");
  const sessionsAfterReopen = (await step({ kind: "listing" })).sessions;

  // The dead generation: the daemon is killed outright, so its locator entry is left behind (no withdraw).
  const locatorBefore = await readLocator();
  const killed = { pid: daemon.pid, digest: locatorBefore.generations[0]?.digest ?? null };
  daemon.kill("SIGKILL");
  await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
  const locatorAfterKill = await readLocator();
  await delay(6_000);
  const coreDead = await snapshot("core-dead", { probe: false });
  const listingDead = await step({ kind: "listing" });
  capture(path.join(shots, "coreDead.png"));

  // The next generation: publishes itself and, in doing so, drops the dead entry.
  daemon = await startDaemon("daemon-2.log");
  await delay(1_500);
  const locatorAfterRestart = await readLocator();
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  const reached = await waitForReach("reached", 90_000);
  await delay(6_000);
  const runtimeReturned = await snapshot("runtime-returned");
  const listingReturned = await step({ kind: "listing" });
  // The roster is the whole machine's: only this journey's conversations are judged, the operator's own are not.
  const journeyNatives = [plusRow?.native, typedRow?.native, host.nativeId].filter(Boolean);
  const rosterReturnedAll = probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]);
  const rosterReturned = {
    live: (rosterReturnedAll.live ?? []).filter((native) => journeyNatives.includes(native)),
    focusable: (rosterReturnedAll.focusable ?? []).filter((native) => journeyNatives.includes(native)),
    machineLive: (rosterReturnedAll.live ?? []).length,
  };
  // The window this generation never saw has to register itself with it; how long that takes is measured.
  const windowsReturned = await waitForWindows(1, 45_000);
  capture(path.join(shots, "runtimeReturned.png"));

  // The window restarts on the new generation: what it persisted must not bring a dead owner back.
  await stopViewer();
  viewer = await launchViewer(2);
  await waitForReach("reached", 90_000);
  const restartedAt = Date.now();
  // The stored conversations come back with the provider catalogue, which a fresh window reads after it reaches
  // the Core; the time that takes is measured rather than assumed.
  const windowRestarted = await snapshotFor("window-restarted", 90_000, (rows) => rows.filter((row) => row.presence === "stored").length >= 3);
  const storedRestoredMs = Date.now() - restartedAt;
  await delay(3_000);
  const listingRestarted = await step({ kind: "listing" });
  const windowsRestarted = await waitForWindows(1, 30_000);
  activate();
  await delay(500);
  capture(path.join(shots, "windowRestarted.png"));

  const stored = (rows) => rows.filter((row) => row.presence === "stored");
  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    owners,
    ownersProved: owners.plus?.presence === "hosted" && owners.typed?.presence === "hosted" && owners.desktop?.presence === "external" && owners.desktop?.canFocus === true,
    typedMirror: typed,
    rosterGoneAfterExits: rosterGone,
    ownersEndedClean: staleRows(ownersEndedSettled).length === 0 && ownersEnded.length >= 3,
    ownersEndedStale: staleRows(ownersEndedSettled),
    reopened,
    reopenedRow: reopenedRow ? facts(reopenedRows, reopenedRow.key) : null,
    reopenedEndedClean: reopenedRow !== null && staleRows(reopenedEnded).length === 0,
    sessionsAfterReopen,
    killed,
    locatorAfterKill: locatorAfterKill.generations.map((g) => ({ digest: g.digest.slice(0, 12), pid: g.processId, draining: g.draining })),
    staleLocatorAfterKill: locatorAfterKill.generations.some((g) => g.processId === killed.pid),
    coreDeadReach: listingDead.coreReach,
    noStaleWhileDead: staleRows(coreDead).length === 0,
    staleWhileDead: staleRows(coreDead),
    locatorAfterRestart: locatorAfterRestart.generations.map((g) => ({ digest: g.digest.slice(0, 12), pid: g.processId, draining: g.draining })),
    prunedByNextGeneration: locatorAfterRestart.generations.length === 1 && locatorAfterRestart.generations[0].processId !== killed.pid,
    reachedAfterRestart: reached,
    runtimeReturnedListing: { coreReach: listingReturned.coreReach, warnings: listingReturned.warnings, incomplete: listingReturned.incomplete, sessions: listingReturned.sessions },
    rosterReturned,
    windowsReturned,
    windowReregistered: windowsReturned.count === 1,
    noStaleAfterRuntimeReturn: staleRows(runtimeReturned).length === 0 && listingReturned.warnings.length === 0 && rosterReturned.live.length === 0 && !listingReturned.sessions.some((s) => s.hot),
    staleAfterRuntimeReturn: staleRows(runtimeReturned),
    windowRestartedListing: { coreReach: listingRestarted.coreReach, warnings: listingRestarted.warnings, incomplete: listingRestarted.incomplete, sessions: listingRestarted.sessions },
    windowsRestarted,
    storedRestoredMs,
    noStaleAfterWindowRestart: staleRows(windowRestarted).length === 0 && listingRestarted.warnings.length === 0 && !listingRestarted.sessions.some((s) => s.hot),
    staleAfterWindowRestart: staleRows(windowRestarted),
    storedConversationsKept: stored(windowRestarted).length >= 3 && stored(windowRestarted).every((row) => row.canOpen),
    storedAfterRestart: stored(windowRestarted).map((row) => ({ key: row.key, title: row.title, canOpen: row.canOpen, blocked: row.blocked })),
    snapshots: timeline.length,
    timeline: timeline.map((entry) => ({
      label: entry.label,
      atMs: entry.atMs,
      rows: entry.rows.map((row) => `${row.key}${row.open ? "*" : ""}<${row.presence}${row.live ? ",live" : ""}${row.canStop ? ",stop" : ""}${row.canFocus ? ",focus" : ""}${row.blocked === RUNNING_ELSEWHERE ? ",elsewhere" : row.blocked === PROCESS_STATUS_UNAVAILABLE ? ",unconfirmed" : ""}>`),
      terminals: entry.terminals,
      warnings: entry.warnings,
    })),
  })}\n`);
} catch (error) {
  failed = true;
  process.stdout.write(`kept for inspection: ${temporary}\n`);
  throw error;
} finally {
  if (viewer) await stopViewer();
  for (const pid of new Set(ownedRoots)) {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", windowsHide: true });
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  if (daemonLog) await daemonLog.close();
  for (const name of ["daemon-1.log", "daemon-2.log"]) {
    try {
      const said = await readFile(path.join(temporary, name), "utf8");
      process.stdout.write(`${name} said:\n${said.split(/\r?\n/).filter(Boolean).slice(-25).join("\n")}\n`);
    } catch { /* nothing was said */ }
  }
  if (!failed) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

// A row that still claims a process, an action on one, or one of the two blocked sentences.
function staleRows(rows) {
  return rows
    .filter((row) => row.live || row.canStop || row.canFocus
      || ["hosted", "supervised", "starting", "external", "unconfirmed"].includes(row.presence)
      || row.blocked === RUNNING_ELSEWHERE || row.blocked === PROCESS_STATUS_UNAVAILABLE
      || row.key.startsWith("started:") || row.key.startsWith("terminal:"))
    .map((row) => ({ key: row.key, presence: row.presence, live: row.live, canStop: row.canStop, canFocus: row.canFocus, blocked: row.blocked }));
}

function facts(rows, key) {
  const row = rows.find((candidate) => candidate.key === key) ?? null;
  return row ? { key: row.key, presence: row.presence, live: row.live, canOpen: row.canOpen, canStop: row.canStop, canFocus: row.canFocus, blocked: row.blocked, native: row.native, open: row.open } : null;
}

function inJourney(row) {
  const folder = row.workspace.toLowerCase();
  return folder === project.toLowerCase() || folder === hostFolder.toLowerCase();
}

function latestRows() {
  return timeline[timeline.length - 1]?.rows ?? [];
}

async function snapshot(label, options = {}) {
  const answer = await step({ kind: "rows" });
  const rows = answer.rows.filter(inJourney);
  let terminals = [];
  let warnings = [];
  if (options.probe !== false) {
    try {
      const listing = probeJson(["terminals-list", runtrolHome, identity, generationDigest]);
      terminals = listing.terminals.map((terminal) => ({ id: terminal.terminalId.slice(-12), origin: terminal.origin, native: terminal.nativeSessionId ?? null, state: terminal.processState }));
      warnings = listing.warnings ?? [];
    } catch (error) {
      warnings = [`probe: ${error instanceof Error ? error.message.slice(0, 160) : String(error)}`];
    }
  }
  timeline.push({ label, atMs: answer.atMs, rows, terminals, warnings });
  return rows;
}

async function snapshotFor(label, durationMs, until) {
  const deadline = Date.now() + durationMs;
  let rows = [];
  while (Date.now() < deadline) {
    rows = await snapshot(label);
    if (until(rows)) break;
    await delay(400);
  }
  return rows;
}

async function waitForReach(wanted, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  let last = null;
  while (Date.now() < deadline) {
    last = (await step({ kind: "listing" })).coreReach;
    if (last === wanted) return true;
    await delay(1_000);
  }
  process.stdout.write(`core reach stayed ${last}\n`);
  return false;
}

// How many windows the current generation's registry lists, waited for up to `deadlineMs` to reach `wanted`.
async function waitForWindows(wanted, deadlineMs) {
  const started = Date.now();
  let count = 0;
  while (Date.now() - started < deadlineMs) {
    count = probeJson(["windows-list", runtrolHome, identity, generationDigest]).windows.length;
    if (count >= wanted) break;
    await delay(1_000);
  }
  return { count, waitedMs: Date.now() - started };
}

async function readLocator() {
  try {
    return JSON.parse(await readFile(path.join(runtrolHome, "runtime.locator.json"), "utf8"));
  } catch (error) {
    return { generations: [], unreadable: error instanceof Error ? error.message.slice(0, 120) : String(error) };
  }
}

async function startDaemon(logName) {
  if (daemonLog) await daemonLog.close();
  daemonLog = await open(path.join(temporary, logName), "w");
  const child = spawn(core, ["daemon"], { env: daemonEnvironment, stdio: ["ignore", daemonLog.fd, daemonLog.fd], windowsHide: true });
  await delay(800);
  return child;
}

async function launchViewer(number) {
  const coordination = path.join(temporary, `coordination-${number}`);
  await mkdir(coordination, { recursive: true });
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
  const launched = { child, coordination, title: "deadgen-viewer", steps: 0 };
  viewer = launched;
  await waitForPublished("viewer-ready.json", 120_000);
  return launched;
}

async function stopViewer() {
  if (!viewer) return;
  try { await publish(`viewer-step-${viewer.steps + 1}.json`, { kind: "done" }); } catch { /* the window may be gone */ }
  await delay(500);
  if (viewer.child.exitCode === null && viewer.child.signalCode === null) viewer.child.kill("SIGKILL");
  await terminateExactProcesses(userData, null);
  viewer = null;
}

async function step(body) {
  viewer.steps += 1;
  await publish(`viewer-step-${viewer.steps}.json`, body);
  return waitForPublished(`viewer-done-${viewer.steps}.json`, 180_000);
}

async function waitForFresh(before, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  let last = null;
  while (Date.now() < deadline) {
    last = probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]);
    const fresh = (last.live ?? []).filter((native) => !before.has(native));
    if (fresh.length > 0 && fresh.every((native) => (last.focusable ?? []).includes(native))) return last;
    await delay(3_000);
  }
  return last ?? { live: [], attachable: [], focusable: [], active: [] };
}

async function waitForGone(natives, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const roster = probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]);
    if (natives.every((native) => !(roster.live ?? []).includes(native))) return true;
    await delay(2_000);
  }
  return false;
}

function processTable() {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name | ConvertTo-Csv -NoTypeInformation"], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  const table = new Map();
  for (const line of ran.stdout.split(/\r?\n/).slice(1)) {
    const cells = line.split(",").map((cell) => cell.replace(/^"|"$/g, ""));
    if (cells.length < 3 || !/^\d+$/.test(cells[0])) continue;
    table.set(Number(cells[0]), { parent: Number(cells[1]), name: cells[2] });
  }
  return table;
}

function descendantsOf(table, root) {
  const found = [];
  const queue = [root];
  while (queue.length > 0) {
    const parent = queue.shift();
    for (const [pid, entry] of table) {
      if (entry.parent === parent && !found.includes(pid)) { found.push(pid); queue.push(pid); }
    }
  }
  return found;
}

function pressKeys(match, keys) {
  const args = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-Keys", keys];
  if (match.title) args.push("-TitleMatch", match.title);
  if (match.family) args.push("-CommandLineMatch", match.family);
  if (match.pid) args.push("-ProcessId", String(match.pid));
  const pressed = spawnSync("powershell.exe", args, { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  return pressed.status === 0 && pressed.stdout.includes("pressed ");
}

function pressViewer(keys) {
  return pressKeys({ title: viewer.title, family: userData }, keys);
}

function activate() {
  pressViewer("{F16}");
}

function capture(outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", viewer.title, "-OutPath", outPath, "-CommandLineMatch", userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

// The visible top-level windows whose title contains `title`, as `{ pid, title }`.
function windowsTitled(title) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "list-windows.ps1")], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  return ran.stdout.split(/\r?\n/)
    .map((line) => { const at = line.indexOf("|"); return at > 0 ? { pid: Number(line.slice(0, at)), title: line.slice(at + 1) } : null; })
    .filter((window) => window !== null && window.title.includes(title));
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
  try { listing = await readdir(viewer.coordination); } catch (error) { listing = [`unreadable: ${String(error).slice(0, 120)}`]; }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms; heartbeat ${JSON.stringify(alive)}; coordination ${JSON.stringify(listing)}`);
}

async function tryReadPublished(name) {
  try {
    const value = JSON.parse(await readFile(path.join(viewer.coordination, name), "utf8"));
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) return null;
    throw error;
  }
}

async function publish(name, value) {
  await writeFile(path.join(viewer.coordination, name), JSON.stringify(value), "utf8");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
