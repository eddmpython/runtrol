// The row click journey (`STATE-04`): a row click always opens, shows the conversation where it runs, or says in
// the row's own words why it cannot be opened. It never does nothing.
//
// Two isolated real VS Code windows (alpha, beta) on one isolated Runtime, one project filed in both, and one
// provider on the desktop in Windows Terminal. Every click is made from beta and its effect is read from beta's
// own report (the tab it opened, the reveal the Runtime answered, the sentence it explained with), from alpha's
// report of its active terminal, and from the desktop (which window holds the foreground). Rows clicked: a
// managed conversation alpha started from its `+` (opens here), a conversation alpha runs in an ordinary terminal
// as an observed mirror (shown in alpha), one on the desktop that no window hosts (its window is brought forward),
// the stored conversation after alpha stopped the first (reopens here), and the same conversation while alpha's
// stop is still under way (explains). Prints one `RUNTROL_ROW_CLICK {json}` line.
//
// Usage: node tooling/row-click-eye.mjs [--keep-shots] [--provider=claude]
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

const MARKER = "RUNTROL_ROW_CLICK ";
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
if (!program) throw new Error(`${providerName} is not on this machine's search path`);
const wt = spawnSync("where.exe", ["wt.exe"], { encoding: "utf8", windowsHide: true }).stdout.split(/\r?\n/).map((l) => l.trim()).find(Boolean);
if (!wt) throw new Error("Windows Terminal (wt.exe) is not installed on this machine");

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-click-"));
const shots = path.join(executionRoot, "runtrol-click-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const project = path.join(temporary, "click-project");
const stamp = `runtrol-state04-${process.pid}`;
const host = { folder: project, title: `${stamp}-wt`, shell: null, owner: null, nativeId: null };

let daemon = null;
let daemonLog = null;
let failed = false;
let executable = null;
let generationDigest = null;
const windows = [];
const ownedRoots = [];
const clicks = [];
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
      JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": `click-${role}` }),
      "utf8",
    );
    roots.push(workspace);
    windows.push({ role, userData, extensions, workspace, title: `click-${role}`, steps: 0, child: null, sessionId: null, pid: null });
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
  for (const window of windows) {
    await step(window, { kind: "addProject", folder: project });
    window.pid = windowsTitled(`[Extension Development Host] ${window.title}`)[0]?.pid ?? null;
  }

  // Row 1, managed: alpha's own `+`. Beta's click opens it here.
  await step(alpha, { kind: "startFresh", provider: providerName, workspace: project });
  await waitFor("alpha's placeholder and its running terminal", 30_000, async () => {
    const rows = await projectRows(alpha);
    return rows.some((row) => row.key.startsWith("started:")) && terminals().some((t) => t.origin === "Owned" && t.processState === "Running");
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
  const managed = await waitFor("the managed conversation in both windows", 90_000, async () => {
    const [a, b] = await Promise.all([hostedRow(alpha), hostedRow(beta)]);
    return a && b && a.key === b.key ? b : null;
  });
  const managedClick = await clickFrom(beta, managed.key, "managed");
  await delay(1_500);
  managedClick.openedHere = (await rowByKey(beta, managed.key))?.open === true;
  managedClick.effect = managedClick.openedHere ? "opened here" : null;
  capture(beta, path.join(shots, "managed-beta.png"));
  await step(beta, { kind: "closeTab", key: managed.key });

  // Row 2, observed: alpha runs the provider by absolute path in an ordinary terminal. Beta's click shows it in alpha.
  await step(alpha, { kind: "start", label: "mirror", commandLine: `& '${program.replace(/'/g, "''")}'`, cwd: project });
  await step(alpha, { kind: "type", label: "mirror", keys: ["[B", "\r"], gapMs: 1_500 });
  await delay(6_000);
  await step(alpha, { kind: "type", label: "mirror", keys: ["Reply with the single word ok.", "\r"], gapMs: 800 });
  const mirror = await waitFor("beta to list the mirror with its owner", 90_000, async () => {
    const rows = await projectRows(beta);
    return rows.find((row) => row.origin === "observedMirror" && row.key.startsWith("chat:")) ?? null;
  });
  await step(alpha, { kind: "showOther" });
  activate(beta);
  await delay(500);
  const foregroundBeforeMirror = foregroundWindow();
  const mirrorClick = await clickFrom(beta, mirror.key, "observed");
  await delay(1_500);
  mirrorClick.foregroundAfter = foregroundWindow();
  mirrorClick.alphaActiveTerminal = (await step(alpha, { kind: "report" })).activeTerminalName;
  mirrorClick.effect = mirrorClick.reveal?.delivered ? "shown in its own window" : null;
  mirrorClick.foregroundBefore = foregroundBeforeMirror;
  mirrorClick.foregroundIsAlpha = Number(mirrorClick.foregroundAfter.split(" ")[0]) === alpha.pid;
  capture(alpha, path.join(shots, "observed-alpha.png"));

  // Row 3, focus only: the provider on the desktop in Windows Terminal. Beta's click brings that window forward.
  const before = new Set(probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]).live ?? []);
  {
    const processesBefore = processTable();
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
    pressPid(host.owner, "{DOWN}{ENTER}");
    await delay(6_000);
    pressPid(host.owner, "Reply with the single word ok.{ENTER}");
    const roster = await waitForFresh(before, 90_000);
    host.nativeId = (roster.live ?? []).find((native) => !before.has(native)) ?? null;
  }
  const desktopKey = host.nativeId ? `chat:${encodeURIComponent(providerName)}:${encodeURIComponent(host.nativeId)}` : null;
  if (!desktopKey) throw new Error("the desktop provider never appeared in the roster");
  await waitFor("beta to list the desktop conversation as focusable", 60_000, async () => (await rowByKey(beta, desktopKey))?.canFocus === true);
  minimizeWindow(host.owner);
  await delay(800);
  activate(beta);
  await delay(500);
  const desktopClick = await clickFrom(beta, desktopKey, "focus-only");
  await delay(1_500);
  desktopClick.foregroundAfter = foregroundWindow();
  desktopClick.foregroundIsHost = Number(desktopClick.foregroundAfter.split(" ")[0]) === host.owner;
  desktopClick.effect = desktopClick.reveal?.delivered && desktopClick.foregroundIsHost ? "its window brought forward" : null;
  capture(beta, path.join(shots, "focus-beta.png"));

  // Row 4, exited: alpha stops the managed conversation; the stored row reopens here on beta's click.
  await step(alpha, { kind: "stopRow", key: managed.key });
  await waitFor("the stopped conversation to be stored in beta", 30_000, async () => (await rowByKey(beta, managed.key))?.presence === "stored");
  activate(beta);
  await delay(400);
  const exitedClick = await clickFrom(beta, managed.key, "exited");
  const reopened = await waitFor("the stored conversation to reopen in beta", 90_000, async () => {
    const row = await rowByKey(beta, managed.key);
    return row?.presence === "hosted" && row.open ? row : null;
  });
  exitedClick.reopenedHere = reopened !== null;
  exitedClick.effect = exitedClick.reopenedHere ? "reopened here" : null;
  await delay(6_000);
  capture(beta, path.join(shots, "exited-beta.png"));

  // Row 5, stopping: alpha stops the reopened conversation and beta clicks at once. The click either explains
  // that the process is still stopping or, if the stop already finished, reopens the stored row; it never does
  // nothing.
  await step(alpha, { kind: "stopRow", key: managed.key });
  const stoppingClick = await clickFrom(beta, managed.key, "stopping");
  await delay(3_000);
  const afterStopping = await rowByKey(beta, managed.key);
  stoppingClick.rowAfter = afterStopping ? { presence: afterStopping.presence, open: afterStopping.open, stopping: afterStopping.stopping } : null;
  stoppingClick.effect = stoppingClick.explanation && /waiting for it to exit/u.test(stoppingClick.explanation)
    ? "explained: stopping"
    : afterStopping?.open ? "reopened here (the stop had already finished)" : null;
  capture(beta, path.join(shots, "stopping-beta.png"));
  if (afterStopping?.open) {
    await step(alpha, { kind: "stopRow", key: managed.key });
    await waitFor("the conversation to be stored again", 30_000, async () => (await rowByKey(beta, managed.key))?.presence === "stored");
  }

  // The desktop provider and the mirror end the way a person ends them.
  pressPid(host.owner, "/exit{ENTER}");
  await step(alpha, { kind: "exit", label: "mirror", keys: ["/exit", "\r"], gapMs: 800 });
  await waitForGone([host.nativeId, mirror.native].filter(Boolean), 60_000);
  pressPid(host.owner, "exit{ENTER}");

  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    clicks,
    everyClickDidSomething: clicks.every((click) => click.effect !== null),
    managedOpensHere: managedClick.openedHere === true,
    observedShownInOwner: mirrorClick.reveal?.delivered === true && mirrorClick.foregroundIsAlpha === true,
    focusOnlyBringsOwnerForward: desktopClick.reveal?.delivered === true && desktopClick.foregroundIsHost === true,
    exitedReopensHere: exitedClick.reopenedHere === true,
    stoppingExplainsOrReopens: stoppingClick.effect !== null,
    stoppingWasExplained: stoppingClick.effect === "explained: stopping",
    noClickFailed: clicks.every((click) => !String(click.outcome).startsWith("failed")),
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
  for (const pid of new Set(ownedRoots)) {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", windowsHide: true });
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

// One click from a window, recorded with everything the window and the Runtime said about it.
async function clickFrom(window, key, kind) {
  const answer = await step(window, { kind: "click", key });
  const record = {
    kind,
    key,
    facts: answer.facts,
    outcome: answer.outcome,
    clickedMs: answer.clickedMs,
    reveal: answer.reveal,
    explanation: answer.explanation,
    activeTerminalName: answer.activeTerminalName,
    effect: null,
  };
  clicks.push(record);
  return record;
}

function terminals() {
  return probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
}

async function projectRows(window) {
  const answer = await step(window, { kind: "rows" });
  return answer.rows.filter((row) => row.workspace.toLowerCase() === project.toLowerCase());
}

async function hostedRow(window) {
  return (await projectRows(window)).find((row) => row.key.startsWith("chat:") && row.presence === "hosted" && row.origin === "owned") ?? null;
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

async function step(window, body) {
  window.steps += 1;
  await publish(`${window.role}-step-${window.steps}.json`, body);
  return waitForPublished(`${window.role}-done-${window.steps}.json`, 180_000);
}

function pressKeys(args, keys) {
  const pressed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"), "-Keys", keys, ...args], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  return pressed.status === 0 && pressed.stdout.includes("pressed ");
}

function press(window, keys) {
  return pressKeys(["-TitleMatch", window.title, "-CommandLineMatch", window.userData], keys);
}

function pressPid(pid, keys) {
  return pressKeys(["-ProcessId", String(pid)], keys);
}

function activate(window) {
  press(window, "{F16}");
}

function capture(window, outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-TitleMatch", window.title, "-OutPath", outPath, "-CommandLineMatch", window.userData], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

function foregroundWindow() {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "foreground-window.ps1")], { encoding: "utf8", timeout: 15_000, windowsHide: true });
  return ran.stdout.trim();
}

function minimizeWindow(pid) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "minimize-window.ps1"), "-ProcessId", String(pid)], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${ran.stdout}${ran.stderr}`.trim() + "\n");
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
