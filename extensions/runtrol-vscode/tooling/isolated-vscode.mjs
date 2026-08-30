import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, mkdirSync, realpathSync } from "node:fs";
import { readdir } from "node:fs/promises";
import path from "node:path";

import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";

import { extensionInstallPrefix } from "./extension-manifest.mjs";
import { descendantPids, normalizedExecutable, processRows } from "./process-identity.mjs";

export const TESTED_VSCODE_VERSION = "1.132.1";

export const isolatedProfileSettings = Object.freeze({
  "chat.agentHost.enabled": false,
  // The built-in Copilot chat is not part of the product under test. Left at its defaults, a fresh profile opens
  // it in the secondary side bar with focus and puts its sign-in button in the title bar; typed keys then land
  // in its composer (measured 2026-08-21: a palette command typed by the harness was submitted to Copilot, which
  // answered with "Sign in to use GitHub Copilot"), and every picture carries a panel nobody asked for.
  "chat.commandCenter.enabled": false,
  "workbench.secondarySideBar.defaultVisibility": "hidden",
  "extensions.autoCheckUpdates": false,
  "extensions.autoUpdate": false,
  "telemetry.telemetryLevel": "off",
  "workbench.enableExperiments": false,
  "workbench.startupEditor": "none",
  // Headless gates exercise extension behavior, not editor tokenization or terminal GPU rendering. Both workers
  // load through vscode-file URLs that Electron can reject in a headless archive, stalling an otherwise healthy
  // Extension Host and adding hundreds of milliseconds to unrelated event-loop measurements.
  "editor.experimental.asyncTokenization": false,
  "terminal.integrated.gpuAcceleration": "off",
});

export const isolatedLaunchArguments = Object.freeze([
  "--disable-extension",
  "GitHub.copilot-chat",
  "--disable-updates",
]);

// Automated Extension Host gates still need Chromium and the VS Code API, but they do not need an operator-facing
// window. Visual eye passes deliberately omit these arguments and remain the only tests allowed on the desktop.
export const quietExtensionTestArguments = Object.freeze([
  "--headless",
  "--disable-gpu",
]);

/// Variables that tell a process it belongs to a VS Code extension host, and must not reach the one under test.
///
/// `ELECTRON_RUN_AS_NODE` is the one that matters. Electron reads it before anything else and starts as plain
/// Node, which makes the workbench never load and the first positional argument be treated as a script to run.
/// Launching a test VS Code from a terminal inside VS Code therefore failed with `Cannot find module
/// <the workspace path>`, and the stack said `runMain`: the workspace folder had been handed to Node as an entry
/// point. Nothing in the message names the cause, and three gates were red for it.
///
/// The `VSCODE_` group is stripped for the same reason one step further out. `VSCODE_IPC_HOOK` and `VSCODE_PID`
/// name the outer instance's pipe and process, and a child that reads them talks to the operator's own window
/// instead of the isolated one this harness built. That is the opposite of isolation.
///
/// This is where it belongs rather than in each caller: the module that promises an isolated VS Code is the one
/// that owes the promise.
const HOST_IDENTITY_VARIABLES = Object.freeze(["ELECTRON_RUN_AS_NODE"]);

/// Whether a variable names the outer extension host rather than the one being launched.
function namesTheOuterHost(name) {
  return HOST_IDENTITY_VARIABLES.includes(name) || name.startsWith("VSCODE_");
}

/// A copy of an environment with the outer extension host's identity removed.
export function withoutHostIdentity(baseEnvironment = process.env) {
  const environment = { ...baseEnvironment };
  for (const name of Object.keys(environment)) {
    if (namesTheOuterHost(name)) delete environment[name];
  }
  return environment;
}

// Removed from this process, not only from the copies it hands out, and removed on import rather than by each
// harness remembering to ask.
//
// `@vscode/test-electron` builds the launcher's environment as `process.env` first and the caller's environment
// on top. Deleting a key from the copy therefore removes nothing: the value underneath survives the merge and
// reaches Electron anyway. That is why stripping the copy fixed the harnesses that spawn VS Code themselves and
// left the ones going through `runTests` failing in exactly the same way as before.
//
// On import because the only reason to import this module is to launch an isolated VS Code, and the cost of one
// harness forgetting is three gates red with a message that names a folder and never mentions an environment.
for (const name of Object.keys(process.env)) {
  if (namesTheOuterHost(name)) delete process.env[name];
}

export function isolatedRuntimeState(root, baseEnvironment = process.env) {
  const environment = withoutHostIdentity(baseEnvironment);
  const canonicalRoot = realpathSync.native(root);
  let home;
  if (process.platform === "win32") {
    environment.LOCALAPPDATA = canonicalRoot;
    home = path.join(canonicalRoot, "runtrol");
  } else if (process.platform === "darwin") {
    environment.HOME = canonicalRoot;
    environment.CFFIXED_USER_HOME = canonicalRoot;
    configureMacOSKeychain(canonicalRoot, environment, baseEnvironment);
    home = path.join(canonicalRoot, "Library", "Application Support", "runtrol");
  } else {
    environment.XDG_STATE_HOME = canonicalRoot;
    home = path.join(canonicalRoot, "runtrol");
  }
  environment.RUNTROL_HOME = home;
  return { environment, home };
}

function configureMacOSKeychain(root, environment, baseEnvironment) {
  const keychain = baseEnvironment.RUNTROL_TEST_MACOS_KEYCHAIN;
  const password = baseEnvironment.RUNTROL_TEST_MACOS_KEYCHAIN_PASSWORD;
  if (!keychain || !password) return;

  mkdirSync(path.join(root, "Library", "Preferences"), { recursive: true });
  for (const arguments_ of [
    ["list-keychains", "-d", "user", "-s", keychain],
    ["default-keychain", "-d", "user", "-s", keychain],
    ["unlock-keychain", "-p", password, keychain],
  ]) {
    const configured = spawnSync("/usr/bin/security", arguments_, {
      env: environment,
      encoding: "utf8",
      windowsHide: true,
    });
    if (configured.status !== 0) {
      throw new Error(
        `isolated macOS keychain setup failed: ${configured.error?.message ?? `exit ${String(configured.status)}`}\n`
        + `${configured.stdout ?? ""}${configured.stderr ?? ""}`,
      );
    }
  }
}

export function isolatedExtensionTestArguments(options) {
  return [
    options.workspace,
    ...isolatedLaunchArguments,
    ...(options.visual ? [] : quietExtensionTestArguments),
    "--disable-extensions",
    `--user-data-dir=${options.userData}`,
    `--extensions-dir=${options.extensions}`,
    "--no-sandbox",
    "--disable-gpu-sandbox",
    "--disable-updates",
    "--skip-welcome",
    "--skip-release-notes",
    "--no-cached-data",
    "--disable-workspace-trust",
    `--extensionTestsPath=${options.testEntry}`,
    `--extensionDevelopmentPath=${options.extensionRoot}`,
  ];
}

export async function acquireVSCode(cachePath) {
  const executable = await downloadAndUnzipVSCode({
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || TESTED_VSCODE_VERSION,
    cachePath,
  });
  const [cli] = resolveCliArgsFromVSCodeExecutablePath(executable);
  return { executable, cli };
}

function installExtension(cli, source, userData, extensions) {
  const installed = spawnSync(
    cli,
    [
      "--user-data-dir",
      userData,
      "--extensions-dir",
      extensions,
      "--install-extension",
      source,
      "--force",
    ],
    {
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
      shell: process.platform === "win32",
    },
  );
  if (installed.status !== 0) {
    throw new Error(
      `extension installation failed: ${installed.error?.message ?? `exit ${String(installed.status)}`}\n`
      + `${installed.stdout ?? ""}${installed.stderr ?? ""}`,
    );
  }
}

export function installVSIX(cli, archive, userData, extensions) {
  installExtension(cli, archive, userData, extensions);
}

export function installMarketplaceExtension(cli, identifier, userData, extensions) {
  installExtension(cli, identifier, userData, extensions);
}

export function uninstallExtension(cli, identifier, userData, extensions) {
  const uninstalled = spawnSync(
    cli,
    [
      "--user-data-dir",
      userData,
      "--extensions-dir",
      extensions,
      "--uninstall-extension",
      identifier,
    ],
    {
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
      shell: process.platform === "win32",
    },
  );
  if (uninstalled.status !== 0) {
    throw new Error(
      `extension uninstall failed: ${uninstalled.error?.message ?? `exit ${String(uninstalled.status)}`}\n`
      + `${uninstalled.stdout ?? ""}${uninstalled.stderr ?? ""}`,
    );
  }
}

export function runInstalledExtensionTest(options) {
  return runTests({
    vscodeExecutablePath: options.vscodeExecutablePath,
    extensionDevelopmentPath: options.verifierRoot,
    extensionTestsPath: options.testEntry,
    extensionTestsEnv: options.environment,
    launchArgs: [
      options.workspace,
      ...isolatedLaunchArguments,
      ...quietExtensionTestArguments,
      "--disable-workspace-trust",
      "--skip-welcome",
      `--user-data-dir=${options.userData}`,
      `--extensions-dir=${options.extensions}`,
    ],
  });
}

export async function findInstalledExtension(root, expectedVersion) {
  const entries = await readdir(root, { withFileTypes: true });
  const matches = entries.filter(
    (entry) => entry.isDirectory() && entry.name.startsWith(extensionInstallPrefix),
  );
  if (!expectedVersion) {
    if (matches.length !== 1) {
      throw new Error(`expected one isolated Runtrol Studio installation, found ${matches.length}`);
    }
    return path.join(root, matches[0].name);
  }
  const expected = matches.filter((entry) => entry.name.includes(`-${expectedVersion}`));
  if (expected.length !== 1) {
    throw new Error(
      `expected one Runtrol Studio ${expectedVersion} directory, found ${expected.length} among ${matches.length}`,
    );
  }
  return path.join(root, expected[0].name);
}

/// Where the extension puts the bundled Core it was given: content-named, never overwritten (the same
/// rule `src/core/managedCore.ts` follows; the sixteen-digit name is that module's contract).
export async function managedCoreImage(userData, extensionIdentifier, bundledCore) {
  const digest = await fileDigest(bundledCore);
  const stem = `runtrol-${digest.slice(0, 16)}`;
  return path.join(
    userData,
    "User",
    "globalStorage",
    extensionIdentifier,
    "core",
    process.platform === "win32" ? `${stem}.exe` : stem,
  );
}

export async function fileDigest(file) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(file)) {
    digest.update(chunk);
  }
  return digest.digest("hex");
}

export async function terminateExactProcesses(marker, executable) {
  const expected = executable ? normalizedExecutable(executable) : "";
  const normalizedMarker = process.platform === "win32"
    ? marker.toLocaleLowerCase("en-US")
    : marker;
  const matchingIdentities = () => {
    const rows = processRows();
    const roots = rows.filter((row) => {
      const command = process.platform === "win32"
        ? row.command.toLocaleLowerCase("en-US")
        : row.command;
      return command.includes(normalizedMarker)
        || (expected && normalizedExecutable(row.executable) === expected);
    });
    const pids = new Set(roots.map((row) => row.pid));
    for (const root of roots) {
      for (const pid of descendantPids(rows, root.pid)) {
        pids.add(pid);
      }
    }
    return rows.filter((row) => pids.has(row.pid) && row.pid !== process.pid);
  };

  const identities = matchingIdentities();
  signalExactProcesses(identities, "SIGTERM");
  let survivors = await waitForExactProcesses(identities, 5_000);
  if (survivors.length > 0) {
    signalExactProcesses(survivors, "SIGKILL");
    survivors = await waitForExactProcesses(identities, 5_000);
  }
  const late = matchingIdentities();
  if (late.length > 0) {
    signalExactProcesses(late, "SIGKILL");
    await waitForExactProcesses(late, 5_000);
  }
  const unique = new Map();
  for (const identity of [...survivingIdentities(identities), ...survivingIdentities(late)]) {
    unique.set(identity.pid, identity);
  }
  survivors = [...unique.values()];
  if (survivors.length > 0) {
    throw new Error(
      `isolated process cleanup left exact PIDs ${survivors.map((row) => row.pid).join(", ")}`,
    );
  }
}

// Ownership proof for a tree we spawned ourselves. The caller holds the root PID, so every
// descendant of its exact process generation is ours by construction and no name matching is involved. A stale
// parent PID from an older Windows process cannot make an older system process part of this tree.
export function ownedTreeIdentities(rootPid) {
  const rows = processRows();
  const pids = descendantPids(rows, rootPid);
  pids.add(rootPid);
  return rows.filter((row) => pids.has(row.pid) && row.pid !== process.pid);
}

// Terminates a tree captured earlier by ownedTreeIdentities. Killing a root does not reap its
// children on Windows, and once the root dies its orphans are no longer reachable by descent, so
// the identities have to be captured while the tree is alive and terminated from that snapshot.
export async function terminateCapturedIdentities(identities) {
  const owned = identities.filter((identity) => identity.pid !== process.pid);
  if (owned.length === 0) {
    return;
  }
  signalExactProcesses(owned, "SIGTERM");
  let survivors = await waitForExactProcesses(owned, 5_000);
  if (survivors.length > 0) {
    signalExactProcesses(survivors, "SIGKILL");
    survivors = await waitForExactProcesses(survivors, 5_000);
  }
  if (survivors.length > 0) {
    throw new Error(
      `owned process cleanup left exact PIDs ${survivors.map((row) => row.pid).join(", ")}`,
    );
  }
}

function signalExactProcesses(identities, signal) {
  for (const identity of identities) {
    try {
      process.kill(identity.pid, signal);
    } catch (error) {
      // Windows can report EPERM after a process has entered kernel teardown but before enumeration stops
      // returning its row. The bounded exact-identity wait below remains the authority: a real survivor is retried
      // with SIGKILL and then fails cleanup, while an already exiting process converges without a false failure.
      if (!isConvergentSignalError(error)) {
        throw error;
      }
    }
  }
}

export function isConvergentSignalError(error, platform = process.platform) {
  return error?.code === "ESRCH" || (platform === "win32" && error?.code === "EPERM");
}

async function waitForExactProcesses(identities, milliseconds) {
  const deadline = Date.now() + milliseconds;
  let survivors = survivingIdentities(identities);
  while (survivors.length > 0 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    survivors = survivingIdentities(identities);
  }
  return survivors;
}

function survivingIdentities(identities) {
  const current = new Map(processRows().map((row) => [row.pid, row]));
  return identities.filter((identity) => {
    const row = current.get(identity.pid);
    return row
      && row.command === identity.command
      && normalizedExecutable(row.executable) === normalizedExecutable(identity.executable)
      && (
        !Number.isFinite(row.startedAt)
        || !Number.isFinite(identity.startedAt)
        || row.startedAt === identity.startedAt
      );
  });
}
