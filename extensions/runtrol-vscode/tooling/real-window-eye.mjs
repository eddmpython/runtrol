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
import { spawn, spawnSync } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runTests } from "@vscode/test-electron";
import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  isolatedLaunchArguments,
  isolatedProfileSettings,
  isolatedRuntimeState,
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
const outDir = path.resolve(process.env.RUNTROL_EYE_OUT || path.join(os.tmpdir(), "runtrol-eye"));
await mkdir(outDir, { recursive: true });

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`production extension build failed:\n${bundled.stdout}${bundled.stderr}`);
}

const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-eye-window-"));
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

const environment = runtimeState.environment;

let daemon = null;
let daemonStderr = "";
try {
  await mkdir(extensionUnderTestRoot, { recursive: true });
  await Promise.all([
    cp(path.join(extensionRoot, "package.json"), path.join(extensionUnderTestRoot, "package.json")),
    cp(path.join(extensionRoot, "dist"), path.join(extensionUnderTestRoot, "dist"), { recursive: true }),
    cp(path.join(extensionRoot, "resources"), path.join(extensionUnderTestRoot, "resources"), { recursive: true }),
  ]);
  await mkdir(path.join(userData, "User"), { recursive: true });
  await mkdir(extensions, { recursive: true });
  await mkdir(runtrolHome, { recursive: true });
  await writeFile(workspaceFile, JSON.stringify({ folders: [{ path: folder }] }), "utf8");
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

  daemon = spawn(core, ["daemon"], { env: environment, stdio: ["ignore", "ignore", "pipe"], windowsHide: true });
  daemon.stderr.setEncoding("utf8").on("data", (chunk) => {
    daemonStderr += chunk;
  });
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
    entryPoints: [path.join(extensionRoot, "src", "integration", `${process.env.RUNTROL_EYE_ENTRY || "realWindowEye"}.test.ts`)],
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
    RUNTROL_EYE_FOLDER: folder,
    RUNTROL_EYE_PROVIDER: providerId,
    ...(process.env.RUNTROL_EYE_PROMPT ? { RUNTROL_EYE_PROMPT: process.env.RUNTROL_EYE_PROMPT } : {}),
    ...(process.env.RUNTROL_EYE_TABS ? { RUNTROL_EYE_TABS: process.env.RUNTROL_EYE_TABS } : {}),
  };

  await Promise.all([
    runTests({
      cachePath: path.join(os.tmpdir(), "runtrol-vscode-test-cache"),
      extensionDevelopmentPath: extensionUnderTestRoot,
      extensionTestsPath: testEntry,
      extensionTestsEnv: testEnvironment,
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
  ]);
  const result = JSON.parse(await readFile(resultPath, "utf8"));
  process.stdout.write(`RUNTROL_EYE ${JSON.stringify({ ...result, outDir })}\n`);
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
        await writeFile(`${resultPath}.captured.${pose}`, "1", "utf8");
      }
    }
    await delay(300);
  }
  throw new Error("the eye pass never completed");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
