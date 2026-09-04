// The row identity journey (`EXT-07`): one command generation is one row, and a placeholder promotes in place.
//
// One isolated real VS Code window on one isolated Runtime, one project folder. Two distinct provider commands
// start under that project: the first through the sidebar's own `+` path (a placeholder row and its tab at once, the
// Runtime open still pending), the second typed by name into an ordinary terminal in that folder (the transparent
// shim brokers it). The sidebar is snapshotted every few hundred milliseconds from before the first start until
// after both exits, and every snapshot is judged: the project never lists more rows than commands started, the
// placeholder is gone the moment its terminal row exists, the open tab sits on exactly one row, the conversation
// row takes the terminal row's place once the provider names it, and no terminal or placeholder row is left after
// exit. Prints one `RUNTROL_ROW_IDENTITY {json}` line.
//
// Usage: node tooling/row-identity-eye.mjs [--keep-shots] [--managed=claude] [--typed=codex]
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

const MARKER = "RUNTROL_ROW_IDENTITY ";
const argument = (name, fallback) => (process.argv.find((word) => word.startsWith(`--${name}=`)) ?? `--${name}=${fallback}`).slice(name.length + 3);
const managedProvider = argument("managed", "claude");
const typedProvider = argument("typed", "codex");
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
for (const name of [managedProvider, typedProvider]) {
  const found = spawnSync("where.exe", [name], { encoding: "utf8", windowsHide: true });
  if (!found.stdout.trim()) throw new Error(`${name} is not on this machine's search path`);
}

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-identity-"));
const shots = path.join(executionRoot, "runtrol-identity-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const project = path.join(temporary, "ident-project");

let daemon = null;
let daemonLog = null;
let failed = false;
let viewer = null;
let executable = null;
let generationDigest = null;
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
  // The daemon's own operational stderr is kept: binding conflicts and ancestry failures are said only there.
  daemonLog = await open(path.join(temporary, "daemon.log"), "w");
  daemon = spawn(core, ["daemon"], { env: daemonEnvironment, stdio: ["ignore", daemonLog.fd, daemonLog.fd], windowsHide: true });
  await delay(500);
  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));

  const userData = path.join(temporary, "viewer-user");
  const extensions = path.join(temporary, "viewer-extensions");
  const workspace = path.join(temporary, "viewer-project");
  await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": "identity-viewer" }),
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
  viewer = { child, userData, title: "identity-viewer", steps: 0 };
  await waitForPublished("viewer-ready.json", 120_000);
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  await delay(4_000);
  const step = async (body) => {
    viewer.steps += 1;
    await publish(`viewer-step-${viewer.steps}.json`, body);
    return waitForPublished(`viewer-done-${viewer.steps}.json`, 180_000);
  };
  // Only this project's rows matter, and among them only the ones a launch can produce: a placeholder, a terminal
  // row, a live conversation, or an open tab. Stored conversations of earlier runs are not this journey's.
  const inProject = (row) => row.workspace.toLowerCase() === project.toLowerCase()
    && (row.key.startsWith("started:") || row.key.startsWith("terminal:") || (row.native !== null && row.live) || row.open);
  let commandsStarted = 0;
  const snapshot = async (label, extra = {}) => {
    const answer = await step({ kind: "rows" });
    const rows = answer.rows.filter(inProject);
    const listing = probeJson(["terminals-list", runtrolHome, identity, generationDigest]);
    const terminals = listing.terminals;
    timeline.push({
      label,
      atMs: answer.atMs,
      commandsStarted,
      rows,
      terminals: terminals.map((terminal) => ({ id: terminal.terminalId, origin: terminal.origin, native: terminal.nativeSessionId ?? null, state: terminal.processState })),
      warnings: listing.warnings ?? [],
      nativeChatCount: answer.nativeChatCount ?? null,
      publishFailure: answer.publishFailure ?? null,
      updatePayload: answer.updatePayload ?? null,
      registeredWindows: probeJson(["windows-list", runtrolHome, identity, generationDigest]).windows.length,
      ...extra,
    });
    return rows;
  };
  const snapshotFor = async (label, durationMs, until = null) => {
    const deadline = Date.now() + durationMs;
    let rows = [];
    while (Date.now() < deadline) {
      rows = await snapshot(label);
      if (until && until(rows)) break;
      await delay(300);
    }
    return rows;
  };
  await step({ kind: "addProject", folder: project });
  await snapshot("baseline");

  // First command: the `+` path. The placeholder and its tab exist before the Runtime has answered.
  const fresh = await step({ kind: "startFresh", provider: managedProvider, workspace: project });
  commandsStarted = 1;
  const placeholderAtStart = fresh.rows.filter(inProject);
  timeline.push({ label: "fresh-answer", atMs: Date.now(), commandsStarted, rows: placeholderAtStart, terminals: [] });
  // The placeholder stands, with the tab, until the provider names the conversation; the Runtime's own terminal
  // runs underneath it and never surfaces as a second row (measured 2026-09-05).
  const placeholderKey = placeholderAtStart.find((row) => row.key.startsWith("started:"))?.key ?? null;
  await snapshotFor("promoting", 20_000, () => (timeline[timeline.length - 1]?.terminals ?? []).some((terminal) => terminal.origin === "Owned" && terminal.state === "Running"));
  const ownedTerminal = (timeline[timeline.length - 1]?.terminals ?? []).find((terminal) => terminal.origin === "Owned") ?? null;
  activate(viewer.title, viewer.userData);
  await delay(500);
  capture(viewer.title, viewer.userData, path.join(shots, "managedHosted.png"));
  // The provider's first-run question and one message, typed into the focused tab the way a person does. The tab
  // is focused by clicking the placeholder's own row.
  if (placeholderKey) await step({ kind: "click", key: placeholderKey });
  await delay(1_000);
  pressKeys(viewer.title, viewer.userData, "{DOWN}{ENTER}");
  await delay(6_000);
  pressKeys(viewer.title, viewer.userData, "Reply with the single word ok.{ENTER}");
  const named = (row) => row.key.startsWith("chat:") && row.hostedKey !== null && row.open;
  // Where the naming stalls, if it stalls: the Runtime binds the terminal to the conversation, the window's read of
  // the provider catalogue lists it, and the list joins the two. Each is asked on its own.
  const naming = [];
  const chatRows = await snapshotFor("naming", 90_000, (rows) => {
    const bound = (timeline[timeline.length - 1]?.terminals ?? []).find((terminal) => terminal.origin === "Owned" && terminal.native !== null) ?? null;
    if (bound && naming.length < 40) {
      naming.push({ atMs: Date.now(), native: bound.native, pending: "asked" });
    }
    return rows.some(named);
  });
  const boundNative = (timeline[timeline.length - 1]?.terminals ?? []).find((terminal) => terminal.origin === "Owned" && terminal.native !== null)?.native ?? null;
  const listedAnswer = boundNative ? await step({ kind: "listed", provider: managedProvider, native: boundNative }) : null;
  // The same question put to the Runtime directly: whether its catalogue for this provider lists the conversation.
  // Asked both ways the window asks: the whole machine first, then the project folder.
  const daemonLists = (root) => {
    const ran = spawnSync(probe, ["native-list", runtrolHome, identity, generationDigest, managedProvider, root], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
    const text = `${ran.stdout}${ran.stderr}`.trim();
    let catalogue = null;
    try { catalogue = JSON.parse(text.split("\n").pop()); } catch { catalogue = null; }
    return {
      status: ran.status,
      listed: catalogue ? (catalogue.sessions ?? []).some((session) => session.nativeSessionId === boundNative) : null,
      count: catalogue ? (catalogue.sessions ?? []).length : null,
      nextCursor: catalogue?.nextCursor ?? null,
      coverage: catalogue?.coverage ?? null,
      answer: catalogue ? null : text.slice(-400),
    };
  };
  const daemonListsBound = boundNative ? { machineWide: daemonLists("-"), projectRoot: daemonLists(project) } : null;
  capture(viewer.title, viewer.userData, path.join(shots, "managedNamed.png"));
  const managedChat = chatRows.find(named) ?? null;
  const managedHostedKey = managedChat?.hostedKey ?? null;

  // Second command: typed by name into an ordinary terminal in the project folder, brokered by the shim.
  const typed = await step({ kind: "start", label: "typed", commandLine: typedProvider, cwd: project });
  commandsStarted = 2;
  timeline.push({ label: "typed-answer", atMs: Date.now(), commandsStarted, rows: [], terminals: [], mirror: typed });
  const typedRows = await snapshotFor("typed-settling", 12_000);
  capture(viewer.title, viewer.userData, path.join(shots, "typedStarted.png"));
  const typedTerminalRows = typedRows.filter((row) => row.key.startsWith("terminal:") && row.key !== managedHostedKey);

  // Both exit the way a person ends them: the managed one through its own exit command in its tab, the typed one
  // through two interrupts in its terminal, which is then disposed.
  // The managed conversation's row as it is now: the named row if the provider named it, else its placeholder.
  const rowsNow = await snapshot("before-exit");
  const managedKey = rowsNow.find((row) => row.key.startsWith("chat:") && row.hostedKey !== null)?.key
    ?? rowsNow.find((row) => row.key.startsWith("started:"))?.key
    ?? null;
  if (managedKey) {
    await step({ kind: "click", key: managedKey });
    await delay(800);
    pressKeys(viewer.title, viewer.userData, "/exit{ENTER}");
  }
  await step({ kind: "exit", label: "typed", keys: ["\u0003", "\u0003"], gapMs: 800 });
  const finalRows = await snapshotFor("after-exit", 30_000, (rows) => !rows.some((row) => row.key.startsWith("terminal:") || row.key.startsWith("started:") || row.live));
  capture(viewer.title, viewer.userData, path.join(shots, "afterExit.png"));

  // Judgement over every snapshot: the project never lists more launch rows than commands started, and one tab.
  const overflows = [];
  const openCounts = [];
  let placeholderBesideItsTerminal = false;
  for (const entry of timeline) {
    if (entry.rows.length > entry.commandsStarted) overflows.push({ label: entry.label, rows: entry.rows.map((row) => row.key) });
    openCounts.push(entry.rows.filter((row) => row.open).length);
    const placeholder = entry.rows.some((row) => row.key.startsWith("started:"));
    const promoted = managedChat !== null && entry.rows.some((row) => row.key === managedChat.key);
    if (placeholder && promoted) placeholderBesideItsTerminal = true;
  }
  const bindingMs = (() => {
    const named = timeline.find((entry) => entry.terminals.some((terminal) => terminal.native !== null));
    const first = timeline.find((entry) => entry.label === "fresh-answer");
    return named && first ? named.atMs - first.atMs : null;
  })();
  process.stdout.write(`${MARKER}${JSON.stringify({
    managedProvider,
    typedProvider,
    freshAnswerMs: fresh.startedMs,
    placeholderAtStart: placeholderAtStart.map((row) => `${row.key}${row.open ? "*" : ""}`),
    placeholderKey,
    ownedTerminal,
    boundNative,
    catalogueListsBound: listedAnswer,
    firstRefusedPayload: timeline.find((entry) => entry.updatePayload)?.updatePayload ?? null,
    daemonListsBound,
    managedChat,
    typedMirror: typed,
    typedTerminalRows,
    overflows,
    placeholderBesideItsTerminal,
    maxOpenRows: Math.max(...openCounts),
    nativeBindingMsAfterStart: bindingMs,
    finalRows,
    snapshots: timeline.length,
    placeholderShownAtOnce: placeholderAtStart.some((row) => row.key.startsWith("started:") && row.open),
    placeholderPromotedInPlace: managedChat !== null && managedChat.open && managedChat.hostedKey !== null && !placeholderBesideItsTerminal,
    conversationTookTerminalsPlace: managedChat !== null,
    typedIsOneRow: typedTerminalRows.length === 1 && typedTerminalRows[0].origin === "owned",
    neverMoreRowsThanCommands: overflows.length === 0,
    oneOpenTabAtMost: Math.max(...openCounts) <= 1,
    nothingLeftAfterExit: finalRows.length === 0,
    timeline: timeline.map((entry) => ({ label: entry.label, atMs: entry.atMs, keys: entry.rows.map((row) => `${row.key}${row.open ? "*" : ""}${row.hostedKey && row.hostedKey !== row.key ? `[${row.hostedKey.slice(-12)}]` : ""}`), terminals: entry.terminals, nativeChatCount: entry.nativeChatCount ?? null, publishFailure: entry.publishFailure ?? null, registeredWindows: entry.registeredWindows ?? null })),
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
      process.stdout.write(`daemon said:\n${said.split(/\r?\n/).filter(Boolean).slice(-40).join("\n")}\n`);
    } catch { /* nothing was said */ }
  }
  if (!failed) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

function pressKeys(titleMatch, userData, keys) {
  const pressed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-TitleMatch", titleMatch, "-Keys", keys, "-CommandLineMatch", userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  return pressed.status === 0;
}

function activate(titleMatch, userData) {
  pressKeys(titleMatch, userData, "{F16}");
}

function capture(titleMatch, userData, outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", titleMatch, "-OutPath", outPath, "-CommandLineMatch", userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
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
