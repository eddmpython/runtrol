// The real-window eye pass: photograph the shipped extension in an isolated VS Code window that talks to an
// isolated Runtime, with the REAL coding CLIs on this machine, the REAL folders they ran in, the REAL stored
// conversations they list, and a REAL conversation answered in this repository.
//
// Isolated means: its own user-data and extensions directories, its own RUNTROL_HOME (so its own daemon
// and socket, never the operator's), and exact-PID cleanup of what it started. Real means: PATH is the
// operator's, HOME is the operator's, the CLIs are the installed ones and are logged in as the operator.
//
// Usage: node tooling/real-window-eye.mjs
//   RUNTROL_EYE_FOLDER   the repository to open and converse in (default: this repository)
//   RUNTROL_EYE_PROVIDER the provider to start the conversation with (default: claude)
//   RUNTROL_EYE_OUT      where the PNGs land (default: %TEMP%/runtrol-eye)
//   RUNTROL_TEST_CORE    the runtrol executable (default: target/debug/runtrol[.exe])
//   RUNTROL_EYE_SHELL_ONLY=1 runs only the project-switch and keyboard-back proof, with no provider turn
import { spawn, spawnSync } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runTests } from "@vscode/test-electron";
import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  acquireVSCode,
  isolatedLaunchArguments,
  isolatedProfileSettings,
  isolatedRuntimeState,
  terminateExactProcesses,
  TESTED_VSCODE_VERSION,
} from "./isolated-vscode.mjs";

const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const core = process.env.RUNTROL_TEST_CORE
  ? path.resolve(process.env.RUNTROL_TEST_CORE)
  : path.join(repositoryRoot, "target", "debug", process.platform === "win32" ? "runtrol.exe" : "runtrol");
await stat(core);
const folder = path.resolve(process.env.RUNTROL_EYE_FOLDER || repositoryRoot);
await stat(folder);
const providerId = process.env.RUNTROL_EYE_PROVIDER || "claude";
const eyeEntry = process.env.RUNTROL_EYE_ENTRY || "realWindowEye";
const outDir = path.resolve(process.env.RUNTROL_EYE_OUT || path.join(os.tmpdir(), "runtrol-eye"));
await mkdir(outDir, { recursive: true });

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);
}

const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-eye-window-"));
const mutatingProjectEntries = new Set([
  "fleetEye",
  "missionAutoFlightEye",
  "missionFlightDeckEye",
  "missionMomentumEye",
  "missionRecoveryEye",
  "missionScheduleEye",
  "safeParallelChatEye",
]);
// Focused Mission eyes build and commit their own fixture repository. Their default must live inside this harness's
// owned temporary root, while an explicitly requested folder remains exact and ordinary read-only eyes keep the
// repository default documented above.
const eyeFolder = !process.env.RUNTROL_EYE_FOLDER && mutatingProjectEntries.has(eyeEntry)
  ? path.join(temporary, "project")
  : folder;
const extensionUnderTestRoot = path.join(temporary, "extension");
const testEntry = path.join(temporary, "realWindowEye.test.cjs");
const resultPath = path.join(temporary, "result.json");
const userData = path.join(temporary, "user");
const extensions = path.join(temporary, "extensions");
// The isolated Runtime state: its own home AND its own system state root, because the public Runtime
// locator finds the daemon through the system state root (LOCALAPPDATA on Windows), not through
// RUNTROL_HOME. Measured the first time this harness ran: with only RUNTROL_HOME moved, the extension
// enrolled with the operator's own daemon and self-approved against the isolated one, and the two never
// met. PATH and HOME stay the operator's, so the CLIs are the real, logged-in ones.
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
// A workspace file, so the window title is distinctive: the operator's own windows are also on this
// repository, and the photographer finds its subject by title.
const workspaceFile = path.join(temporary, "runtrol-eye.code-workspace");
const titleMatch = "runtrol-eye (Workspace)";

const environment = { ...runtimeState.environment };
if (eyeEntry === "agentToolsEye") {
  // Agent Tools intentionally changes provider MCP configuration. The focused eye pass receives clean provider
  // homes inside this harness's owned temporary directory, so proving the shipped command can never mutate the
  // operator's own Claude or Codex configuration.
  environment.CLAUDE_CONFIG_DIR = path.join(temporary, "claude");
  environment.CODEX_HOME = path.join(temporary, "codex");
}

let daemon = null;
let daemonStderr = "";
function launchRuntimeDaemon() {
  const child = spawn(core, ["daemon"], {
    env: environment,
    stdio: ["ignore", "ignore", "pipe"],
    windowsHide: true,
  });
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    daemonStderr += chunk;
  });
  return child;
}
try {
  await mkdir(eyeFolder, { recursive: true });
  await mkdir(extensionUnderTestRoot, { recursive: true });
  await Promise.all([
    cp(path.join(extensionRoot, "package.json"), path.join(extensionUnderTestRoot, "package.json")),
    cp(path.join(extensionRoot, "dist"), path.join(extensionUnderTestRoot, "dist"), { recursive: true }),
    cp(path.join(extensionRoot, "resources"), path.join(extensionUnderTestRoot, "resources"), { recursive: true }),
  ]);
  await mkdir(path.join(userData, "User"), { recursive: true });
  await mkdir(extensions, { recursive: true });
  await mkdir(runtrolHome, { recursive: true });
  if (eyeEntry === "agentToolsEye") {
    await Promise.all([
      mkdir(environment.CLAUDE_CONFIG_DIR, { recursive: true }),
      mkdir(environment.CODEX_HOME, { recursive: true }),
    ]);
  }
  await writeFile(workspaceFile, JSON.stringify({ folders: [{ path: eyeFolder }] }), "utf8");
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "workbench.colorTheme": "Default Dark Modern",
      "window.zoomLevel": 0,
    }),
    "utf8",
  );

  daemon = launchRuntimeDaemon();
  await delay(500);
  if (daemon.exitCode !== null) throw new Error(`the isolated Runtime stopped during startup:\n${daemonStderr}`);
  const reached = spawnSync(core, ["endpoint"], { env: environment, encoding: "utf8", timeout: 15_000, windowsHide: true });
  if (reached.status !== 0 || !reached.stdout.trim()) {
    throw new Error(`the isolated Runtime exposed no endpoint:\n${reached.stdout}${reached.stderr}`);
  }
  process.stdout.write(`isolated Runtime at ${reached.stdout.trim()} (home ${runtrolHome}, pid ${daemon.pid})\n`);

  await build({
    // RUNTROL_EYE_ENTRY names another entry under src/integration (a focused probe); the poses it announces
    // are photographed the same way.
    entryPoints: [path.join(extensionRoot, "src", "integration", `${eyeEntry}.test.ts`)],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });

  const testEnvironment = {
    ...environment,
    RUNTROL_TEST_CORE: core,
    RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1",
    RUNTROL_VSCODE_RESULT: resultPath,
    RUNTROL_EYE_FOLDER: eyeFolder,
    RUNTROL_EYE_PROVIDER: providerId,
    RUNTROL_EYE_NODE: process.execPath,
    ...(process.env.RUNTROL_EYE_PROMPT ? { RUNTROL_EYE_PROMPT: process.env.RUNTROL_EYE_PROMPT } : {}),
    ...(process.env.RUNTROL_EYE_TABS ? { RUNTROL_EYE_TABS: process.env.RUNTROL_EYE_TABS } : {}),
  };

  const runEyeWindow = async (extraEnvironment = {}) => Promise.all([
    runTests({
      cachePath: path.join(os.tmpdir(), "runtrol-vscode-test-cache"),
      extensionDevelopmentPath: extensionUnderTestRoot,
      extensionTestsPath: testEntry,
      extensionTestsEnv: { ...testEnvironment, ...extraEnvironment },
      launchArgs: [
        workspaceFile,
        ...isolatedLaunchArguments,
        "--disable-extensions",
        "--disable-workspace-trust",
        `--user-data-dir=${userData}`,
        `--extensions-dir=${extensions}`,
      ],
      version: process.env.RUNTROL_TEST_VSCODE_VERSION || TESTED_VSCODE_VERSION,
      vscodeExecutablePath: process.env.RUNTROL_TEST_VSCODE_EXECUTABLE || undefined,
    }),
    photograph(),
    restartRuntimeWhenRequested(),
  ]);
  if (process.env.RUNTROL_EYE_SHELL_ONLY === "1") {
    const back = await backProof(testEnvironment);
    if (!back.switched || !back.returned) {
      throw new Error(`the project switch and keyboard back proof did not complete: ${JSON.stringify(back)}`);
    }
    process.stdout.write(`RUNTROL_EYE ${JSON.stringify({ stage: "complete", back, outDir })}\n`);
  } else {
    if (eyeEntry === "missionScheduleEye") {
      await runEyeWindow({ RUNTROL_MISSION_SCHEDULE_PHASE: "schedule" });
      const scheduled = JSON.parse(await readFile(resultPath, "utf8"));
      if (scheduled.stage !== "complete" || scheduled.phase !== "scheduled") {
        throw new Error(`the first Studio window did not freeze a schedule: ${JSON.stringify(scheduled)}`);
      }
      const studioClosedUnixMs = Date.now();
      if (!(studioClosedUnixMs < scheduled.dueUnixMs)) {
        throw new Error("the first Studio window did not close before the reviewed due instant");
      }
      process.stdout.write(
        `closed first Studio window at ${studioClosedUnixMs}, before due ${scheduled.dueUnixMs}\n`,
      );
      await delay(Math.max(0, scheduled.dueUnixMs - Date.now()) + 6_000);
      if (!daemon || daemon.exitCode !== null) {
        throw new Error("the isolated Runtime stopped while Studio was closed");
      }
      await writeFile(resultPath, JSON.stringify({ stage: "launching-observer" }), "utf8");
      await runEyeWindow({
        RUNTROL_MISSION_SCHEDULE_PHASE: "observe",
        RUNTROL_MISSION_SCHEDULE_ID: scheduled.missionId,
        RUNTROL_MISSION_SCHEDULE_DUE: String(scheduled.dueUnixMs),
        RUNTROL_MISSION_STUDIO_CLOSED: String(studioClosedUnixMs),
      });
    } else {
      await runEyeWindow();
    }
    const result = JSON.parse(await readFile(resultPath, "utf8"));
    // The window switch and the back key, proved with real windows from outside: a switch reloads the extension
    // host, so nothing inside the test runner survives to press the next key. A plain (non-test) isolated window
    // is opened on this repository, Ctrl+K Ctrl+Shift+P picks another project, the title changes, Ctrl+K Ctrl+B
    // brings it back, the title changes back; both photographed.
    const back = eyeEntry === "realWindowEye"
      ? await backProof(testEnvironment)
      : { skipped: `focused ${eyeEntry} eye pass` };
    if (eyeEntry === "realWindowEye" && (!back.switched || !back.returned)) {
      throw new Error(`the project switch and keyboard back proof did not complete: ${JSON.stringify(back)}`);
    }
    process.stdout.write(`RUNTROL_EYE ${JSON.stringify({ ...result, back, outDir })}\n`);
  }
} catch (error) {
  const progress = await readFile(resultPath, "utf8").then((text) => JSON.parse(text)).catch(() => null);
  if (progress?.failure) {
    throw new Error(`the eye pass failed at ${progress.stage}: ${progress.failure}\n${progress.stack ?? ""}`, { cause: error });
  }
  if (daemonStderr) {
    throw new Error(`the eye pass failed and the isolated Runtime reported:\n${daemonStderr}`, { cause: error });
  }
  throw error;
} finally {
  if (daemon && daemon.exitCode === null) {
    // The isolated Runtime's own sessions were closed by the entry; the daemon itself is this harness's exact
    // child and the only process it terminates.
    const exited = new Promise((resolve) => daemon.once("close", resolve));
    daemon.kill();
    await Promise.race([exited, delay(5_000)]);
  }
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }).catch(() => undefined);
}

/// The switch-and-back proof. Returns what was seen; never throws for a missing second project (it says so).
async function backProof(environment) {
  const defaultOther = path.join(path.dirname(repositoryRoot.replace(/[\\/]$/u, "")), "taxly");
  const other = path.resolve(process.env.RUNTROL_EYE_BACK_FOLDER || defaultOther);
  const otherExists = await stat(other).then((info) => info.isDirectory()).catch(() => false);
  if (!otherExists) return { skipped: `no second project folder at ${other}` };
  const here = repositoryRoot.replace(/[\\/]$/u, "");
  const hereName = path.basename(here);
  const otherName = path.basename(other);
  // Only this harness's own development-host window is ever matched, pressed or photographed: the operator's
  // own VS Code is open on this very repository, and a bare folder name would find it first.
  const hereTitle = `Development Host] ${hereName}`;
  const otherTitle = `Development Host] ${otherName}`;
  const { executable } = await acquireVSCode(path.join(os.tmpdir(), "runtrol-vscode-test-cache"));
  const child = spawn(
    executable,
    [
      here,
      ...isolatedLaunchArguments,
      // What the test runner passes on its own; a plain launch shows the welcome and the account onboarding
      // otherwise, and a modal in front of the editor takes every key (measured: the first press landed on
      // "Welcome to VS Code").
      "--skip-welcome",
      "--skip-release-notes",
      "--disable-extensions",
      "--disable-workspace-trust",
      `--extensionDevelopmentPath=${extensionUnderTestRoot}`,
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensions}`,
    ],
    { env: environment, stdio: "ignore", windowsHide: false, detached: false },
  );
  const seen = { opened: false, switched: false, returned: false, returnedBy: null };
  try {
    seen.opened = await waitForTitle(hereTitle, 90_000);
    if (!seen.opened) return { ...seen, failure: "the window on this repository never appeared" };
    // The extension activates on the first command; the list it offers is read after ready. A breath for both.
    await delay(15_000);
    // Anything modal that still came up (an onboarding the flags do not cover) closes on Escape; a bare
    // Escape on the editor does nothing.
    press(hereTitle, "{ESC}");
    await delay(1_000);
    // The switch goes through the command palette, which opens whatever has focus (a fresh window focuses its
    // chat input, where a chord is that input's); the back step is the key itself, the palette only as the
    // recorded fallback so the result says which worked.
    press(hereTitle, "^+p");
    await delay(1_500);
    press(hereTitle, "Runtrol: Switch Window to Project{ENTER}");
    // The picker reads the project list after the extension is ready. A machine with hundreds of real stored
    // conversations can spend more than eight seconds refreshing them before the command runs, so typing before the
    // picker exists sends the project name into the editor and leaves the later picker untouched. This delay is a
    // bounded shell-evidence allowance, not product waiting: the production command itself remains event-driven.
    await delay(30_000);
    capture(hereTitle, path.join(outDir, "switchPicker.png"));
    press(hereTitle, `${otherName}{ENTER}`);
    seen.switched = await waitForTitle(otherTitle, 60_000);
    if (!seen.switched) capture(hereTitle, path.join(outDir, "switchFailed.png"));
    if (seen.switched) {
      await delay(1_500);
      capture(otherTitle, path.join(outDir, "switched.png"));
      await delay(12_000);
      press(otherTitle, "{ESC}");
      await delay(500);
      press(otherTitle, "^k^b");
      seen.returned = await waitForTitle(hereTitle, 30_000);
      seen.returnedBy = seen.returned ? "key" : null;
      if (!seen.returned) {
        press(otherTitle, "^+p");
        await delay(1_500);
        press(otherTitle, "Runtrol: Back to Previous Project{ENTER}");
        seen.returned = await waitForTitle(hereTitle, 30_000);
        seen.returnedBy = seen.returned ? "palette" : null;
      }
      if (seen.returned) {
        await delay(1_500);
        capture(hereTitle, path.join(outDir, "returned.png"));
      }
    }
    return seen;
  } finally {
    // A project switch can replace the original launcher with a new `Code.exe -n` root whose PID is not a child
    // of the launcher any more. Match the isolated profile and this downloaded executable as well, otherwise the
    // visual pass leaves a desktop singleton that later automated gates attach to.
    if (child.exitCode === null) child.kill();
    await terminateExactProcesses(userData, executable);
  }
}

function press(titleMatch, keys) {
  const pressed = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"),
      "-TitleMatch", titleMatch,
      "-Keys", keys,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${pressed.stdout}${pressed.stderr}`.trim() + "\n");
}

function capture(titleMatch, outPath) {
  const shot = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"),
      "-TitleMatch", titleMatch,
      "-OutPath", outPath,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  process.stdout.write(`${shot.stdout}${shot.stderr}`.trim() + "\n");
}

/// Whether a window whose title contains the text appears within the deadline.
async function waitForTitle(titleMatch, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const found = spawnSync(
      "powershell.exe",
      [
        "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command",
        `(Get-Process | Where-Object { $_.MainWindowTitle -like "*${titleMatch}*" } | Select-Object -First 1).MainWindowTitle`,
      ],
      { encoding: "utf8", timeout: 15_000, windowsHide: true },
    );
    if (found.stdout.trim()) return true;
    await delay(1_000);
  }
  return false;
}

/// Photograph every pose the entry announces, by window title, into the output directory.
async function photograph() {
  const deadline = Date.now() + 15 * 60_000;
  const done = new Set();
  while (Date.now() < deadline) {
    const progress = await readFile(resultPath, "utf8").then((text) => JSON.parse(text)).catch(() => null);
    if (progress && typeof progress.failure === "string") return;
    if (progress && progress.stage === "complete") return;
    if (progress && typeof progress.stage === "string" && progress.stage.startsWith("capture:")) {
      const pose = progress.stage.slice("capture:".length);
      if (!done.has(pose)) {
        done.add(pose);
        const outPath = path.join(outDir, `${pose}.png`);
        const shot = spawnSync(
          "powershell.exe",
          [
            "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
            "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"),
            "-TitleMatch", titleMatch,
            "-OutPath", outPath,
          ],
          { encoding: "utf8", timeout: 30_000, windowsHide: true },
        );
        if (shot.status !== 0) {
          throw new Error(`the ${pose} capture failed:\n${shot.stdout}${shot.stderr}`);
        }
        process.stdout.write(`captured ${pose} -> ${outPath} ${JSON.stringify(progress)}\n`);
        if (eyeEntry === "missionRecoveryEye" && pose === "recoveryCancellation") {
          press(titleMatch, "{ESC}");
        }
        if (eyeEntry === "missionRecoveryEye" && pose === "recoveryConfirmation") {
          // This is the real focused Quick Pick. Press its selected recovery action from outside the Extension Host,
          // the same input boundary the screenshot captured, rather than calling a test-only product method.
          press(titleMatch, "{ENTER}");
        }
        await writeFile(`${resultPath}.captured.${pose}`, "1", "utf8");
      }
    }
    await delay(300);
  }
  throw new Error("the eye pass never completed");
}

/// One focused fault pose kills only this harness's exact Runtime child and starts its successor over the same home.
async function restartRuntimeWhenRequested() {
  if (!new Set(["missionRecoveryEye", "safeParallelChatEye"]).has(eyeEntry)) return;
  const deadline = Date.now() + 10 * 60_000;
  while (Date.now() < deadline) {
    const progress = await readFile(resultPath, "utf8").then((text) => JSON.parse(text)).catch(() => null);
    if (progress && (typeof progress.failure === "string" || progress.stage === "complete")) return;
    if (progress?.stage !== "fault:restartCore") {
      await delay(200);
      continue;
    }
    const previous = daemon;
    if (!previous || previous.exitCode !== null) {
      throw new Error("the isolated Runtime was not alive at the requested restart boundary");
    }
    const exited = new Promise((resolve) => previous.once("close", resolve));
    previous.kill();
    await Promise.race([
      exited,
      delay(5_000).then(() => Promise.reject(new Error("the faulted Runtime did not exit within 5 seconds"))),
    ]);
    daemon = launchRuntimeDaemon();
    await delay(500);
    if (daemon.exitCode !== null) throw new Error(`the successor Runtime stopped during startup:\n${daemonStderr}`);
    const reached = spawnSync(core, ["endpoint"], {
      env: environment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
    if (reached.status !== 0 || !reached.stdout.trim()) {
      throw new Error(`the successor Runtime exposed no endpoint:\n${reached.stdout}${reached.stderr}`);
    }
    process.stdout.write(
      `restarted isolated Runtime from pid ${previous.pid} to pid ${daemon.pid} at ${reached.stdout.trim()}\n`,
    );
    await writeFile(`${resultPath}.restarted`, JSON.stringify({ before: previous.pid, after: daemon.pid }), "utf8");
    return;
  }
  throw new Error(`the ${eyeEntry} eye never requested its Core restart fault`);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
