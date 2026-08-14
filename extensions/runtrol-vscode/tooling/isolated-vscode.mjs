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
  "extensions.autoCheckUpdates": false,
  "extensions.autoUpdate": false,
  "workbench.startupEditor": "none",
});

export const isolatedLaunchArguments = Object.freeze([
  "--disable-updates",
]);

export function isolatedRuntimeState(root, baseEnvironment = process.env) {
  const environment = { ...baseEnvironment };
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

function signalExactProcesses(identities, signal) {
  for (const identity of identities) {
    try {
      process.kill(identity.pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
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
      && normalizedExecutable(row.executable) === normalizedExecutable(identity.executable);
  });
}
