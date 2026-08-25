import { spawn, spawnSync } from "node:child_process";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  extensionIdentifier,
  extensionRoot,
  packageManifest,
} from "./extension-manifest.mjs";
import {
  acquireVSCode,
  fileDigest,
  findInstalledExtension,
  installVSIX,
  isolatedLaunchArguments,
  isolatedProfileSettings,
  isolatedRuntimeState,
  terminateExactProcesses,
  managedCoreImage,
} from "./isolated-vscode.mjs";

if (process.platform !== "win32") {
  throw new Error("the installed safe parallel eye currently photographs the Windows Extension Host");
}
const archive = path.resolve(process.argv[2] ?? "");
if (!process.argv[2]) {
  throw new Error("usage: node tooling/installed-safe-parallel-eye.mjs <platform.vsix>");
}
await access(archive);

const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-installed-safe-"));
const runtimeState = isolatedRuntimeState(temporary);
const userData = path.join(temporary, "user-data");
const extensions = path.join(temporary, "extensions");
const project = path.join(temporary, "project");
const workspaceFile = path.join(temporary, "installed-safe.code-workspace");
const outDir = path.resolve(
  process.env.RUNTROL_INSTALLED_EYE_OUT ?? path.join(os.tmpdir(), "runtrol-installed-safe-eye"),
);
const screenshot = path.join(outDir, "installedSafeParallel.png");
const draftScreenshot = path.join(outDir, "installedSafeParallelDraft.png");
const placementScreenshot = path.join(outDir, "installedSafeParallelPlacement.png");
const titleMatch = "installed-safe (Workspace)";
let bundledCore = null;
let managedCore = null;
let vscodeProcess = null;
let vscodeOutput = "";

try {
  await Promise.all([
    mkdir(path.join(userData, "User"), { recursive: true }),
    mkdir(extensions, { recursive: true }),
    mkdir(project, { recursive: true }),
    mkdir(outDir, { recursive: true }),
  ]);
  await prepareProject(project);
  await writeFile(workspaceFile, JSON.stringify({ folders: [{ path: project }] }), "utf8");
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "workbench.colorTheme": "Default Dark Modern",
      "window.zoomLevel": 0,
    }),
    "utf8",
  );

  const vscode = await acquireVSCode(path.join(os.tmpdir(), "runtrol-vscode-test-cache"));
  installVSIX(vscode.cli, archive, userData, extensions);
  const installed = await findInstalledExtension(extensions, packageManifest.version);
  bundledCore = path.join(installed, "resources", "core", "runtrol.exe");
  await access(bundledCore);
  managedCore = await managedCoreImage(userData, extensionIdentifier, bundledCore);

  vscodeProcess = spawn(
    vscode.executable,
    [
      workspaceFile,
      ...isolatedLaunchArguments,
      "--disable-workspace-trust",
      "--skip-welcome",
      "--new-window",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensions}`,
    ],
    {
      env: runtimeState.environment,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: false,
    },
  );
  for (const stream of [vscodeProcess.stdout, vscodeProcess.stderr]) {
    stream.setEncoding("utf8").on("data", (chunk) => {
      vscodeOutput = `${vscodeOutput}${chunk}`.slice(-16_000);
    });
  }
  vscodeProcess.unref();
  await waitForWindow(titleMatch, 60_000, () => (
    vscodeProcess.exitCode === null
      ? null
      : `VS Code exited ${String(vscodeProcess.exitCode)} before opening its window:\n${vscodeOutput}`
  ));
  await delay(5_000);
  press(titleMatch, "^k^n");
  // Provider verification is intentionally serialized because every probe starts a CLI. Let the second real
  // service finish that bounded discovery before asking the public command to offer it.
  await delay(12_000);
  press(titleMatch, "^+p");
  await delay(500);
  press(titleMatch, "Runtrol: Also Ask Another Service{ENTER}");
  await delay(1_200);
  press(titleMatch, "{ENTER}");
  await delay(800);
  capture(titleMatch, draftScreenshot);
  click(titleMatch, 800, 798);
  await delay(300);
  press(titleMatch, "Reply with exactly: installed isolated parallel complete{ENTER}");
  await delay(800);
  capture(titleMatch, placementScreenshot);
  press(titleMatch, "{ENTER}");

  const registryPath = path.join(runtimeState.home, "isolated-workspaces.json");
  const bound = await waitForBoundWorkspaces(registryPath, 2, 90_000);
  const baseCommit = git(project, "rev-parse", "HEAD");
  const baseCommon = await commonGitDirectory(project);
  const generatedRoot = await realpath(path.join(temporary, ".runtrol-worktrees"));
  const workspacePaths = await Promise.all(bound.map((record) => realpath(record.workspace)));
  if (new Set(workspacePaths.map(pathIdentity)).size !== 2) {
    throw new Error("the installed extension did not create two distinct worktrees");
  }
  for (const workspace of workspacePaths) {
    if (!isInside(generatedRoot, workspace)) throw new Error(`${workspace} is outside ${generatedRoot}`);
    if (git(workspace, "rev-parse", "HEAD") !== baseCommit) {
      throw new Error("an installed isolated chat did not freeze the selected base commit");
    }
    if (await commonGitDirectory(workspace) !== baseCommon) {
      throw new Error("an installed isolated chat is not a linked worktree of the selected project");
    }
  }
  if (git(project, "status", "--porcelain") !== "") {
    throw new Error("the installed parallel chat changed the selected checkout");
  }
  const registryText = await readFile(registryPath, "utf8");
  if (registryText.includes("installed isolated parallel complete")) {
    throw new Error("the installed Core registry stored conversation text");
  }
  await waitForFile(managedCore, 60_000, "the installed extension's managed Core");
  if (await fileDigest(managedCore) !== await fileDigest(bundledCore)) {
    throw new Error("the installed extension did not run the Core bundled in the VSIX");
  }

  await delay(3_000);
  capture(titleMatch, screenshot);
  const dimensions = imageDimensions(screenshot);
  for (const record of bound) {
    runCore(bundledCore, runtimeState.environment, "close", record.session_id, "--now");
  }
  press(titleMatch, "^+p");
  await delay(500);
  press(titleMatch, "Runtrol: Refresh Conversations{ENTER}");
  await waitForReleasedWorkspaces(registryPath, workspacePaths, 90_000);
  if (git(project, "status", "--porcelain") !== "") {
    throw new Error("the installed cleanup changed the selected checkout");
  }

  process.stdout.write(`RUNTROL_INSTALLED_SAFE_PARALLEL ${JSON.stringify({
    archive,
    installed,
    bundledCore,
    managedCore,
    sessions: bound.map((record) => record.session_id),
    distinctWorkspaces: true,
    baseCommit,
    baseUnchanged: true,
    cleanupRemoved: true,
    registryStoredConversation: false,
    screenshot,
    draftScreenshot,
    placementScreenshot,
    viewport: dimensions,
  })}\n`);
} finally {
  if (bundledCore) {
    spawnSync(bundledCore, ["panic"], {
      env: runtimeState.environment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
  }
  if (vscodeProcess?.exitCode === null) vscodeProcess.kill();
  await terminateExactProcesses(temporary, managedCore);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

async function waitForBoundWorkspaces(registryPath, wanted, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    const registry = await readFile(registryPath, "utf8").then(JSON.parse).catch(() => null);
    const bound = registry?.records?.filter((record) => record.state === "bound") ?? [];
    if (bound.length === wanted && bound.every((record) => typeof record.session_id === "string")) return bound;
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${wanted} bound installed worktrees`);
    }
    await delay(250);
  }
}

async function waitForReleasedWorkspaces(registryPath, workspaces, deadlineMs) {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    const registry = await readFile(registryPath, "utf8").then(JSON.parse).catch(() => null);
    const active = registry?.records?.filter((record) => record.state !== "released") ?? [];
    const present = await Promise.all(workspaces.map((workspace) => stat(workspace).then(() => true).catch(() => false)));
    if (active.length === 0 && present.every((value) => !value)) return;
    if (Date.now() > deadline) throw new Error("the installed extension did not release its clean worktrees");
    await delay(250);
  }
}

async function commonGitDirectory(folder) {
  return pathIdentity(await realpath(path.resolve(folder, git(folder, "rev-parse", "--git-common-dir"))));
}

async function prepareProject(folder) {
  git(folder, "init", "--initial-branch=main");
  git(folder, "config", "user.email", "fixture@runtrol.invalid");
  git(folder, "config", "user.name", "Runtrol Fixture");
  await writeFile(path.join(folder, "README.md"), "# Installed safe parallel fixture\n", "utf8");
  git(folder, "add", "--", "README.md");
  git(folder, "commit", "-m", "base fixture");
}

function git(folder, ...arguments_) {
  const result = spawnSync("git", arguments_, { cwd: folder, encoding: "utf8", windowsHide: true });
  if (result.status !== 0) throw new Error(`git ${arguments_.join(" ")} failed:\n${result.stdout}${result.stderr}`);
  return result.stdout.trim();
}

function runCore(core, environment, ...arguments_) {
  const result = spawnSync(core, arguments_, { env: environment, encoding: "utf8", timeout: 30_000, windowsHide: true });
  if (result.status !== 0) throw new Error(`Core ${arguments_.join(" ")} failed:\n${result.stdout}${result.stderr}`);
}

function press(title, keys) {
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "press-keys.ps1"),
      "-TitleMatch", title,
      "-Keys", keys,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`key press failed:\n${result.stdout}${result.stderr}`);
}

function capture(title, outPath) {
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"),
      "-TitleMatch", title,
      "-OutPath", outPath,
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`window capture failed:\n${result.stdout}${result.stderr}`);
}

function click(title, x, y) {
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", path.join(extensionRoot, "tooling", "click-window.ps1"),
      "-TitleMatch", title,
      "-X", String(x),
      "-Y", String(y),
    ],
    { encoding: "utf8", timeout: 30_000, windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`window click failed:\n${result.stdout}${result.stderr}`);
}

async function waitForWindow(title, deadlineMs, stopped = () => null) {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    try {
      press(title, "{ESC}");
      return;
    } catch {
      const failure = stopped();
      if (failure) throw new Error(failure);
      if (Date.now() > deadline) throw new Error(`timed out waiting for the ${title} window`);
      await delay(500);
    }
  }
}

async function waitForFile(file, deadlineMs, label) {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    try {
      await access(file);
      return;
    } catch {
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
      await delay(250);
    }
  }
}

function imageDimensions(file) {
  const bytes = spawnSync(
    "powershell.exe",
    [
      "-NoProfile", "-NonInteractive", "-Command",
      `Add-Type -AssemblyName System.Drawing; $image=[System.Drawing.Image]::FromFile('${file.replaceAll("'", "''")}'); `
      + `try { Write-Output ($image.Width.ToString() + 'x' + $image.Height.ToString()) } finally { $image.Dispose() }`,
    ],
    { encoding: "utf8", timeout: 15_000, windowsHide: true },
  );
  if (bytes.status !== 0) throw new Error(`image dimensions failed:\n${bytes.stdout}${bytes.stderr}`);
  return bytes.stdout.trim();
}

function isInside(parent, child) {
  const relative = path.relative(parent, child);
  return relative.length > 0 && relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}

function pathIdentity(value) {
  const resolved = path.resolve(value);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
