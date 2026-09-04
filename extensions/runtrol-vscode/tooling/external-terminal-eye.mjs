// The arbitrary external terminal journey (`EXT-06`): a real provider running in Windows Terminal and another in a
// classic console window, neither observed by any VS Code window.
//
// One isolated real VS Code window is the viewer on one isolated Runtime. The harness starts the provider twice on
// the desktop, each in its own project folder filed in the viewer, answers the folder-trust question and says one
// thing so each conversation is written down. Each row must read `Focus owner` (focusable, not openable), the click
// must bring the provider's own terminal-host window to the desktop foreground without starting a second provider
// process, the Runtime must hold no terminal for either, and after both providers exit the rows must stop being live
// and offer no focus. Prints one `RUNTROL_EXTERNAL_TERMINAL {json}` line.
//
// Usage: node tooling/external-terminal-eye.mjs [--keep-shots] [--provider=claude|codex]
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

const MARKER = "RUNTROL_EXTERNAL_TERMINAL ";
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

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-external-"));
const shots = path.join(executionRoot, "runtrol-external-eye");
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };
const stamp = `runtrol-ext06-${process.pid}`;
const hosts = [
  { name: "wt", folder: path.join(temporary, "wt-project"), title: `${stamp}-wt` },
  { name: "console", folder: path.join(temporary, "console-project"), title: `${stamp}-console` },
];

let daemon = null;
let failed = false;
let viewer = null;
let executable = null;
let generationDigest = null;
const ownedRoots = [];
try {
  await Promise.all([coordination, runtrolHome, shots, ...hosts.map((host) => host.folder)].map((d) => mkdir(d, { recursive: true })));
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

  const userData = path.join(temporary, "viewer-user");
  const extensions = path.join(temporary, "viewer-extensions");
  const workspace = path.join(temporary, "viewer-project");
  await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({ ...isolatedProfileSettings, "runtrol.corePath": core, "window.title": "external-viewer" }),
    "utf8",
  );
  const roots = [workspace, ...hosts.map((host) => host.folder)];
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
  viewer = { role: "viewer", child, userData, title: "external-viewer", steps: 0 };
  await waitForPublished("viewer-ready.json", 120_000);
  generationDigest = probeJson(["enroll", runtrolHome, core, identity, ...roots]).generation;
  await delay(4_000);
  const step = async (body) => {
    viewer.steps += 1;
    await publish(`viewer-step-${viewer.steps}.json`, body);
    return waitForPublished(`viewer-done-${viewer.steps}.json`, 180_000);
  };
  for (const host of hosts) await step({ kind: "addProject", folder: host.folder });

  // Each provider starts on the desktop, in its own terminal host, with the launching agent's markers already
  // stripped from this process's environment by isolated-vscode.mjs. The shell command carries the stamp so the
  // console window can be found by its process even after the provider retitles it.
  const shellCommand = `$host.UI.RawUI.WindowTitle='${stamp}'; & '${program.replace(/'/g, "''")}'`;
  const before = new Set(probeJson(["native-activity", runtrolHome, identity, generationDigest, providerName]).live ?? []);
  const results = [];
  for (const host of hosts) {
    const processesBefore = processTable();
    if (host.name === "wt") {
      // Not `windowsHide`: the launcher hands its startup state to the terminal it starts, and a hidden launcher
      // gave a Windows Terminal window nobody could see (measured 2026-09-05).
      // No `;` in a Windows Terminal command line: it is the separator between wt subcommands, and a command
      // containing one is split into two tabs (measured 2026-09-05). The tab title comes from `--title` instead.
      spawn(wt, ["-w", "new", "new-tab", "--title", host.title, "--suppressApplicationTitle", "-d", host.folder, "powershell", "-NoExit", "-Command", `& '${program.replace(/'/g, "''")}'`], { detached: true, stdio: "ignore" }).unref();
    } else {
      spawn("cmd.exe", ["/c", "start", `"${host.title}"`, "/D", host.folder, "powershell", "-NoExit", "-Command", shellCommand], { detached: true, stdio: "ignore", shell: false }).unref();
    }
    // A terminal host takes its time to show a window, and the window is the proof: for Windows Terminal it is
    // found by the title the tab keeps (`--suppressApplicationTitle`) and its owning process is whatever process
    // Windows says owns it (measured 2026-09-05: one fresh `WindowsTerminal.exe` here, but a reused one when the
    // operator already has a window open); for a console it is the shell itself.
    let processesAfter = processTable();
    let fresh = [];
    let shell = null;
    let owner = null;
    for (let waited = 0; waited < 25_000 && owner === null; waited += 1_000) {
      await delay(1_000);
      processesAfter = processTable();
      fresh = [...processesAfter.keys()].filter((pid) => !processesBefore.has(pid));
      shell = fresh.find((pid) => processesAfter.get(pid).name.toLowerCase() === "powershell.exe") ?? null;
      if (host.name === "wt") {
        owner = windowsTitled(host.title)[0]?.pid ?? null;
      } else if (shell !== null && windowsOwnedBy([shell]) !== "") {
        owner = shell;
      }
    }
    ownedRoots.push(...fresh.filter((pid) => ["windowsterminal.exe", "powershell.exe", "conhost.exe", "openconsole.exe"].includes(processesAfter.get(pid).name.toLowerCase())));
    host.shell = shell ?? null;
    host.owner = owner;
    // The window is found by the process that owns it, never by title: the provider retitles a console window
    // and a WindowsTerminal window as soon as it starts (measured 2026-09-05).
    process.stdout.write(`windows at launch: ${windowsOwnedBy([owner, shell].filter(Boolean))}\n`);
    if (owner === null) {
      // Nothing is typed anywhere without a proved window. What was there is logged for the next measurement.
      const terminalHosts = fresh.filter((pid) => ["windowsterminal.exe", "openconsole.exe", "conhost.exe"].includes(processesAfter.get(pid).name.toLowerCase()));
      process.stdout.write(`no window for ${host.name}: fresh hosts ${JSON.stringify(terminalHosts.map((pid) => `${processesAfter.get(pid).name}(${pid})`))}; their windows ${windowsOwnedBy(terminalHosts)}; titled ${JSON.stringify(windowsTitled(stamp))}\n`);
      results.push({ host: host.name, shell, owner: null, nativeId: null, focusableAtRoster: false });
      continue;
    }
    const match = { pid: owner };
    // Folder trust defaults to `No, exit` (measured 2026-09-04): one arrow down, Enter; then one short message.
    pressKeys(match, "{DOWN}{ENTER}");
    await delay(6_000);
    pressKeys(match, "Reply with the single word ok.{ENTER}");
    await delay(8_000);
    captureProcess(owner, path.join(shots, `host-${host.name}-after-message.png`));
    const roster = await waitForFresh(providerName, before, 90_000);
    const nativeId = (roster.live ?? []).find((native) => !before.has(native) && !results.some((r) => r.nativeId === native)) ?? null;
    before.add(nativeId);
    results.push({ host: host.name, shell, owner, nativeId, focusableAtRoster: nativeId !== null && (roster.focusable ?? []).includes(nativeId) });
  }

  const terminalsBefore = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;
  activate(viewer.title, viewer.userData);
  await delay(800);
  capture(viewer.title, viewer.userData, path.join(shots, "viewerBeforeClicks.png"));
  const clicks = [];
  for (const result of results) {
    if (result.nativeId === null) { clicks.push({ host: result.host, skipped: "no conversation" }); continue; }
    const rowKey = `chat:${encodeURIComponent(providerName)}:${encodeURIComponent(result.nativeId)}`;
    activate(viewer.title, viewer.userData);
    await delay(500);
    const providersBefore = countProcesses(`${providerName}.exe`);
    // The host window is minimised first, so the click has to restore it and bring it forward: a window that was
    // already in front proves nothing.
    minimizeWindow(result.owner);
    await delay(800);
    const stateBefore = windowState(result.owner);
    const foregroundBefore = foregroundWindow();
    const clicked = await step({ kind: "click", key: rowKey });
    await delay(1_500);
    const foreground = foregroundWindow();
    const stateAfter = windowState(result.owner);
    const foregroundPid = Number(foreground.split(" ")[0]) || 0;
    const table = processTable();
    clicks.push({
      host: result.host,
      rowKey,
      facts: clicked.facts,
      reveal: clicked.reveal,
      clickedMs: clicked.clickedMs,
      foregroundBefore,
      foreground,
      foregroundProcess: table.get(foregroundPid)?.name ?? null,
      // The window that came forward belongs to the provider's own terminal host: the Windows Terminal process for
      // the first, the console shell itself for the second (measured 2026-09-05).
      foregroundIsOwner: foregroundPid === result.owner,
      stateBefore,
      stateAfter,
      // Proved by state, not by guessing: the window was minimised before the click and is restored after it.
      foregroundMoved: stateBefore.split("|")[1] === "True" && stateAfter.split("|")[1] === "False",
      providerProcessesBefore: providersBefore,
      providerProcessesAfter: countProcesses(`${providerName}.exe`),
    });
    capture(viewer.title, viewer.userData, path.join(shots, `viewerAfterClick-${result.host}.png`));
  }
  const terminalsAfter = probeJson(["terminals-list", runtrolHome, identity, generationDigest]).terminals;

  // Both providers exit through their own command; the rows must stop being live and stop offering a focus.
  const endings = [];
  for (const host of hosts) {
    if (!host.owner) continue;
    // The provider's own exit command first; when the keys cannot be delivered (Windows refuses the foreground to
    // the key sender now and then), the provider process under this host's shell is ended by its exact identity,
    // which is the other way a person ends it.
    const typed = pressKeys({ pid: host.owner }, "/exit{ENTER}");
    if (typed) { endings.push({ host: host.name, endedBy: "exit command" }); continue; }
    const table = processTable();
    const under = descendantsOf(table, host.shell).filter((pid) => table.get(pid).name.toLowerCase().startsWith(providerName.toLowerCase()));
    for (const pid of under) spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", windowsHide: true });
    endings.push({ host: host.name, endedBy: `process end ${JSON.stringify(under)}` });
  }
  const gone = await waitForGone(providerName, results.map((r) => r.nativeId).filter(Boolean), 60_000);
  await delay(3_000);
  // The shells close too, so a Windows Terminal window the operator already had open keeps no stray tab.
  for (const host of hosts) {
    if (host.owner) pressKeys({ pid: host.owner }, "exit{ENTER}");
  }
  const after = [];
  for (const result of results) {
    if (result.nativeId === null) continue;
    const rowKey = `chat:${encodeURIComponent(providerName)}:${encodeURIComponent(result.nativeId)}`;
    const facts = await step({ kind: "rowFacts", key: rowKey });
    after.push({ host: result.host, facts: facts.facts });
  }

  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    program,
    hosts: results,
    terminalsBefore: terminalsBefore.length,
    terminalsAfter: terminalsAfter.length,
    clicks,
    exitedFromRoster: gone,
    endings,
    afterExit: after,
    everyRowSaidFocusOwner: clicks.every((c) => c.facts?.canFocus === true && c.facts?.canOpen === false),
    everyClickRaisedTheOwner: clicks.every((c) => c.reveal?.delivered === true && c.foregroundIsOwner && c.foregroundMoved),
    noDuplicateProvider: clicks.every((c) => c.providerProcessesAfter === c.providerProcessesBefore),
    neverHosted: terminalsBefore.length === 0 && terminalsAfter.length === 0,
    rowsStoppedAfterExit: gone && after.length === results.length && after.every((a) => a.facts?.live === false && a.facts?.canFocus === false),
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
  // Only what this run started on the desktop: the terminal hosts and shells recorded at launch, by exact PID.
  for (const pid of new Set(ownedRoots)) {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", windowsHide: true });
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  if (!failed) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  if (!keepShots) await rm(shots, { recursive: true, force: true });
}

async function waitForFresh(provider, before, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  let last = null;
  while (Date.now() < deadline) {
    last = probeJson(["native-activity", runtrolHome, identity, generationDigest, provider]);
    const fresh = (last.live ?? []).filter((native) => !before.has(native));
    process.stdout.write(`roster: fresh ${JSON.stringify(fresh)} focusable ${JSON.stringify(last.focusable ?? [])}\n`);
    if (fresh.length > 0 && fresh.every((native) => (last.focusable ?? []).includes(native))) return last;
    await delay(3_000);
  }
  return last ?? { live: [], attachable: [], focusable: [], active: [] };
}

async function waitForGone(provider, natives, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const roster = probeJson(["native-activity", runtrolHome, identity, generationDigest, provider]);
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

function countProcesses(imageName) {
  const ran = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${imageName}`, "/NH"], { encoding: "utf8", windowsHide: true });
  return ran.stdout.split(/\r?\n/).filter((line) => line.toLowerCase().startsWith(imageName.toLowerCase())).length;
}

function foregroundWindow() {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "foreground-window.ps1")], { encoding: "utf8", timeout: 15_000, windowsHide: true });
  return ran.stdout.trim();
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

function descendantsOf(table, root) {
  const found = [];
  const queue = [root];
  while (queue.length > 0) {
    const parent = queue.shift();
    for (const [pid, process] of table) {
      if (process.parent === parent && !found.includes(pid)) { found.push(pid); queue.push(pid); }
    }
  }
  return found;
}

function activate(titleMatch, userData) {
  pressKeys({ title: titleMatch, family: userData }, "{F16}");
}

function windowState(pid) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "window-state.ps1"), "-ProcessId", String(pid)], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  return ran.stdout.trim();
}

function minimizeWindow(pid) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "minimize-window.ps1"), "-ProcessId", String(pid)], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${ran.stdout}${ran.stderr}`.trim() + "\n");
}

function captureProcess(pid, outPath) {
  const shot = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"), "-ProcessId", String(pid), "-OutPath", outPath], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

// The visible top-level windows whose title contains `title`, as `{ pid, title }`.
function windowsTitled(title) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "list-windows.ps1")], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  return ran.stdout.split(/\r?\n/)
    .map((line) => { const at = line.indexOf("|"); return at > 0 ? { pid: Number(line.slice(0, at)), title: line.slice(at + 1) } : null; })
    .filter((window) => window !== null && window.title.includes(title));
}

// The visible top-level windows these processes own, as `pid|title`, for the log.
function windowsOwnedBy(pids) {
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(extensionRoot, "tooling", "list-windows.ps1")], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  return ran.stdout.split(/\r?\n/).filter((line) => pids.some((pid) => line.startsWith(`${pid}|`))).join(" ; ");
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
