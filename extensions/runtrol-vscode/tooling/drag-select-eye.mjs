// The drag-select eye pass: a provider that switches mouse reporting on, opened as a Runtrol tab in an isolated
// real VS Code window, dragged across by a real pointer, photographed, and answered with Enter.
//
// What it proves, all at once (`terminalTransportIntegrity`, `TERM-05`):
//   - the Core forwards the provider's mouse-mode switch unchanged (the viewer's own capture carries it, and the
//     same program on a bare ConPTY draws the same screen);
//   - the Studio tab still selects on drag, because it takes that one control family out at its own edge;
//   - no mouse report reaches the provider from the drag (the provider echoes the line Enter submits with any
//     ESC made visible, so a leaked report would be on the screen);
//   - the pictures are the judgement of the selection itself.
//
// The provider is the deterministic ACP fixture in its terminal mode, so the pass costs no model turn and runs the
// same on every machine with a desktop. Isolated means its own user data, extensions, Runtime home and system state
// root; PATH for the daemon holds only the fixture. Everything it starts is stopped by exact PID.
//
// Usage: node tooling/drag-select-eye.mjs
//   RUNTROL_EYE_OUT   where the PNGs land (default: <dev-workspace>/runtrol-drag-eye)
//   RUNTROL_EYE_DRAG  the drag in client pixels "x1,y1,x2,y2" (default: 20,58,240,58: across the first row, which
//                     stands under the window's own title bar, measured 2026-09-02 on 1.132.1 at zoom 0)
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

const PROVIDER = "fixture-acp";
const TITLE = "runtrol-drag-select";
const MARKER = "RUNTROL_DRAG_SELECT ";
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const executionRoot = process.platform === "win32"
  ? path.join(requiredEnvironment("LOCALAPPDATA"), "dev-workspace")
  : path.join(os.homedir(), ".local", "share", "dev-workspace");
await mkdir(executionRoot, { recursive: true });
const outDir = path.resolve(process.env.RUNTROL_EYE_OUT || path.join(executionRoot, "runtrol-drag-eye"));
await mkdir(outDir, { recursive: true });
const drag = (process.env.RUNTROL_EYE_DRAG || "20,58,240,58").split(",").map(Number);
if (drag.length !== 4 || drag.some((value) => !Number.isInteger(value) || value < 0)) {
  throw new Error("RUNTROL_EYE_DRAG must be four non-negative integers x1,y1,x2,y2");
}

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
  const built = spawnSync("cargo", ["build", "-p", packageName, kind, name], {
    cwd: repositoryRoot,
    encoding: "utf8",
    windowsHide: true,
  });
  if (built.status !== 0) throw new Error(`cargo build ${packageName} ${name} failed:\n${built.stderr}`);
}
await Promise.all([stat(core), stat(probe), stat(fixture)]);

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);

const temporary = await mkdtemp(path.join(executionRoot, "runtrol-drag-"));
const coordination = path.join(temporary, "coordination");
const testEntry = path.join(temporary, "dragSelectEye.test.cjs");
const userData = path.join(temporary, "user");
const extensions = path.join(temporary, "extensions");
const workspace = path.join(temporary, "workspace");
const identity = path.join(temporary, "identity.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const daemonEnvironment = { ...runtimeState.environment, PATH: path.dirname(fixture) };
const hostEnvironment = {
  ...runtimeState.environment,
  RUNTROL_TEST_CORE: core,
  RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
  RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]),
  RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
  RUNTROL_VSCODE_PROVIDER: PROVIDER,
  RUNTROL_VSCODE_WORKSPACE: workspace,
  RUNTROL_VSCODE_COORDINATION: coordination,
};

let daemon = null;
let daemonStderr = "";
let window = null;
let executable = null;
try {
  await Promise.all([coordination, path.join(userData, "User"), extensions, workspace, path.join(runtrolHome, "providers")]
    .map((directory) => mkdir(directory, { recursive: true })));
  await writeFile(
    path.join(runtrolHome, "providers", `${PROVIDER}.toml`),
    [
      "schema = 1",
      `id = "${PROVIDER}"`,
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
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "window.title": TITLE,
      "workbench.colorTheme": "Default Dark Modern",
      "window.zoomLevel": 0,
      // Zen mode stands the tab alone without leaving the desktop or centring it in margins.
      "zenMode.fullScreen": false,
      "zenMode.centerLayout": false,
      "zenMode.showTabs": "none",
      "zenMode.hideStatusBar": true,
      "zenMode.hideActivityBar": true,
    }),
    "utf8",
  );
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "dragSelectEye.test.ts")],
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
  daemon.stderr.setEncoding("utf8").on("data", (chunk) => {
    daemonStderr += chunk;
  });
  await delay(500);

  ({ executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test")));
  window = spawn(
    executable,
    isolatedExtensionTestArguments({ workspace, userData, extensions, testEntry, extensionRoot, visual: true }),
    { env: hostEnvironment, stdio: "inherit", windowsHide: false },
  );
  const terminal = await waitForPublished("ready.json", 90_000);
  if (!(await waitForTitle(TITLE, 30_000, userData))) throw new Error("the isolated window never appeared by title");
  await delay(1_500);
  capture(TITLE, path.join(outDir, "beforeDrag.png"), userData);
  dragAcross(TITLE, drag, userData);
  await delay(800);
  capture(TITLE, path.join(outDir, "dragged.png"), userData);
  await publish("dragged.json", { drag });
  const { selection } = await waitForPublished("selection.json", 30_000);
  press(TITLE, "{ENTER}", userData);
  await publish("entered.json", {});
  await waitForPublished("echoed.json", 30_000);
  await delay(500);
  capture(TITLE, path.join(outDir, "entered.png"), userData);

  // The same terminal through the public wire, as any Runtime client sees it, and the same program on a bare
  // ConPTY: the mouse-mode switch is in both, and the screens agree row for row.
  runJson(probe, daemonEnvironment, ["enroll", runtrolHome, core, identity, workspace]);
  const viewer = runJson(probe, daemonEnvironment, [
    "screen", runtrolHome, identity, terminal.runtimeGeneration, terminal.terminalId, "1500",
  ]);
  const direct = runJson(probe, daemonEnvironment, ["direct-program", workspace, "1500", "0d", fixture, "--tui"]);
  await publish("captured.json", {});
  const result = await waitForPublished("result.json", 30_000);
  await requireExit(window, "the isolated VS Code window", 30_000);
  window = null;

  const viewerRows = viewer.rows.filter((row) => row.trim() !== "");
  const directRows = direct.rows.filter((row) => row.trim() !== "");
  const echoRows = viewerRows.filter((row) => row.startsWith("echo:"));
  process.stdout.write(`${MARKER}${JSON.stringify({
    selection,
    selectionIsFirstRow: selection.trim() === "acp-fixture terminal ready",
    echoRows,
    noReportReachedTheProvider: echoRows.length === 1 && echoRows[0].trim() === "echo:",
    viewerMouseModeSeen: viewer.mouseModeSeen === true,
    directMouseModeSeen: direct.mouseModeSeen === true,
    viewerRows,
    directRows,
    screensAgree: viewerRows.join("\n") === directRows.join("\n"),
    terminal: result.terminal,
    vscode: result.vscode,
    screenshots: ["beforeDrag.png", "dragged.png", "entered.png"].map((name) => path.join(outDir, name)),
  })}\n`);
} finally {
  if (window && window.exitCode === null && window.signalCode === null) window.kill("SIGKILL");
  await terminateExactProcesses(userData, executable);
  if (daemon && daemon.exitCode === null) {
    spawnSync(core, ["panic"], { env: daemonEnvironment, encoding: "utf8", windowsHide: true, timeout: 30_000 });
    await waitForExit(daemon, 10_000);
    if (daemon.exitCode === null) daemon.kill("SIGKILL");
  }
  if (daemonStderr.trim()) process.stderr.write(`daemon stderr:\n${daemonStderr}\n`);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}

function dragAcross(titleMatch, [x1, y1, x2, y2], commandLineMatch) {
  const dragged = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "click-window.ps1"),
      "-TitleMatch", titleMatch,
      "-X", String(x1),
      "-Y", String(y1),
      "-DragToX", String(x2),
      "-DragToY", String(y2),
      "-CommandLineMatch", commandLineMatch,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${dragged.stdout}${dragged.stderr}`.trim() + "\n");
  if (dragged.status !== 0) throw new Error(`the drag in ${JSON.stringify(titleMatch)} failed`);
}

function press(titleMatch, keys, commandLineMatch) {
  const pressed = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"),
      "-TitleMatch", titleMatch,
      "-Keys", keys,
      "-CommandLineMatch", commandLineMatch,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
  if (pressed.status !== 0) throw new Error(`typing ${JSON.stringify(keys)} into ${JSON.stringify(titleMatch)} failed`);
}

function capture(titleMatch, outPath, commandLineMatch) {
  const shot = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"),
      "-TitleMatch", titleMatch,
      "-OutPath", outPath,
      "-CommandLineMatch", commandLineMatch,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
  if (shot.status !== 0) throw new Error(`capturing ${JSON.stringify(titleMatch)} failed`);
}

async function waitForTitle(titleMatch, deadlineMs, commandLineMatch) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const found = spawnSync(
      "powershell.exe",
      [
        "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", path.join(extensionRoot, "tooling", "find-window.ps1"),
        "-TitleMatch", titleMatch,
        "-CommandLineMatch", commandLineMatch,
      ],
      { encoding: "utf8", timeout: 15_000, windowsHide: true },
    );
    if (found.stdout.trim()) return true;
    await delay(1_000);
  }
  return false;
}

function runJson(program, environment, words) {
  const ran = spawnSync(program, words, { env: environment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (ran.status !== 0) throw new Error(`${path.basename(program)} ${words[0]} failed: ${ran.stderr}${ran.stdout}`);
  return JSON.parse(ran.stdout.trim().split("\n").pop());
}

async function waitForPublished(name, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const failed = await tryReadPublished("failure.json");
    if (failed) throw new Error(`the window failed: ${failed.failure}\n${failed.stack ?? ""}`);
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

async function requireExit(child, label, deadlineMs) {
  await waitForExit(child, deadlineMs);
  if (child.exitCode === null) throw new Error(`${label} did not exit within ${deadlineMs} ms`);
  if (child.exitCode !== 0) throw new Error(`${label} exited as ${child.exitCode}`);
}

function waitForExit(child, deadlineMs) {
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, deadlineMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
