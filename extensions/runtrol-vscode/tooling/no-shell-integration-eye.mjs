// The measurement that decides EXT-05 (nothing is designed from this file, it only finds out what is true).
//
// One isolated real VS Code window with shell integration turned OFF starts a real provider by absolute path in an
// ordinary terminal. The harness then reads, through the public wire and the operating system:
//   1. what the window registry knows about that terminal (shell integration flag, shell process id),
//   2. whether Studio opened a mirror for it (it must not: with no shell integration there is no output stream),
//   3. what the provider's own process roster says is live and attachable,
//   4. whether the live provider process really is a descendant of that terminal's shell.
// Prints one `RUNTROL_NO_SHELL_INTEGRATION {json}` line.
//
// Usage: node tooling/no-shell-integration-eye.mjs [--provider=claude|codex]
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

const MARKER = "RUNTROL_NO_SHELL_INTEGRATION ";
const providerName = (process.argv.find((word) => word.startsWith("--provider=")) ?? "--provider=claude").slice("--provider=".length);
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

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-nosi-"));
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "ownerReveal.test.cjs");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment };

let daemon = null;
let window = null;
let executable = null;
try {
  await Promise.all([coordination, runtrolHome].map((d) => mkdir(d, { recursive: true })));
  // The owner-reveal journey entry already speaks the step protocol this measurement needs.
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

  const userData = path.join(temporary, "alpha-user");
  const extensions = path.join(temporary, "alpha-extensions");
  const workspace = path.join(temporary, "alpha-project");
  await Promise.all([path.join(userData, "User"), extensions, workspace].map((d) => mkdir(d, { recursive: true })));
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "window.title": "nosi-alpha",
      // The whole point of the measurement.
      "terminal.integrated.shellIntegration.enabled": false,
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
        RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]),
        RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
        RUNTROL_VSCODE_ROLE: "alpha",
        RUNTROL_VSCODE_COORDINATION: coordination,
      },
      stdio: "ignore",
      windowsHide: false,
    },
  );
  window = { child, userData, workspace, steps: 0 };
  const ready = await waitForPublished("alpha-ready.json", 120_000);
  const enrolled = probeJson(["enroll", runtrolHome, core, identity, workspace]);
  await delay(6_000);

  const step = async (body) => {
    window.steps += 1;
    await publish(`alpha-step-${window.steps}.json`, body);
    return waitForPublished(`alpha-done-${window.steps}.json`, 180_000);
  };
  await step({ kind: "addProject", folder: workspace });
  // The journey entry waits for shell integration before running the command; with it disabled that wait must fail,
  // which is itself the first measurement. So the command is typed instead, the way a person does.
  const started = await step({ kind: "startTyped", label: "provider", commandLine: `& '${program.replace(/'/g, "''")}'`, settleMs: 25_000 });

  const windows = probeJson(["windows-list", runtrolHome, identity, enrolled.generation]).windows;
  const terminals = probeJson(["terminals-list", runtrolHome, identity, enrolled.generation]).terminals;
  const roster = probeJson(["native-activity", runtrolHome, identity, enrolled.generation, providerName]);
  const report = await step({ kind: "report" });
  const observed = (windows[0]?.terminals ?? []).map((terminal) => ({
    key: terminal.terminalKey,
    name: terminal.name,
    processId: terminal.processId ?? null,
    shellIntegration: terminal.shellIntegration,
    command: terminal.command?.commandLine ?? null,
  }));
  const shellPids = observed.map((terminal) => terminal.processId).filter((pid) => typeof pid === "number");
  const ancestry = shellPids.length ? providerAncestry(providerName, shellPids) : [];

  process.stdout.write(`${MARKER}${JSON.stringify({
    provider: providerName,
    program,
    windowSessionId: (ready.sessionId ?? "").slice(0, 8),
    startStep: started,
    observedTerminals: observed,
    mirroredTerminals: terminals.map((terminal) => ({ providerId: terminal.providerId, origin: terminal.origin })),
    roster,
    ancestry,
    activeTerminal: report.activeTerminalName ?? null,
    rows: report.rowKeys?.length ?? null,
  })}\n`);
} finally {
  if (window) {
    try { await publish(`alpha-step-${window.steps + 1}.json`, { kind: "done" }); } catch { /* the window may be gone */ }
    if (window.child.exitCode === null && window.child.signalCode === null) window.child.kill("SIGKILL");
    await terminateExactProcesses(window.userData, null);
  }
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await new Promise((resolve) => { const timer = setTimeout(resolve, 10_000); daemon.once("exit", () => { clearTimeout(timer); resolve(); }); });
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}

// Every live process of the provider, with its parent chain, and whether any of the window's shells is on it.
function providerAncestry(name, shellPids) {
  const script = [
    "$rows = @()",
    `Get-CimInstance Win32_Process -Filter "Name='${name}.exe'" | ForEach-Object {`,
    "  $chain = @(); $current = $_.ParentProcessId",
    "  for ($i = 0; $i -lt 12 -and $current -gt 0; $i++) {",
    "    $parent = Get-CimInstance Win32_Process -Filter \"ProcessId = $current\" -ErrorAction SilentlyContinue",
    "    if (-not $parent) { break }",
    "    $chain += \"$($parent.ProcessId):$($parent.Name)\"",
    "    $current = $parent.ParentProcessId",
    "  }",
    "  $rows += [pscustomobject]@{ pid = $_.ProcessId; chain = $chain }",
    "}",
    "$rows | ConvertTo-Json -Depth 4 -Compress",
  ].join("\n");
  const ran = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8", timeout: 60_000, windowsHide: true });
  let rows = [];
  try {
    const parsed = JSON.parse(ran.stdout.trim() || "[]");
    rows = Array.isArray(parsed) ? parsed : [parsed];
  } catch {
    return [{ error: `${ran.stdout}${ran.stderr}`.slice(0, 300) }];
  }
  return rows.map((row) => ({
    pid: row.pid,
    chain: row.chain,
    underWindowShell: (row.chain ?? []).some((entry) => shellPids.includes(Number(String(entry).split(":")[0]))),
  }));
}

function probeJson(words) {
  const ran = spawnSync(probe, words, { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (ran.status !== 0) throw new Error(`handoverProbe ${words[0]} failed: ${ran.stderr}${ran.stdout}`);
  return JSON.parse(ran.stdout.trim().split("\n").pop());
}

async function waitForPublished(name, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const failed = await tryReadPublished("alpha-failure.json");
    if (failed) throw new Error(`alpha failed: ${failed.failure}`);
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
