import { spawnSync } from "node:child_process";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { build } from "esbuild";

import {
  extensionIdentifier,
  extensionRoot,
  packageManifest,
} from "./extension-manifest.mjs";
import {
  acquireVSCode,
  fileDigest,
  findInstalledExtension,
  installMarketplaceExtension,
  installVSIX,
  isolatedProfileSettings,
  isolatedRuntimeState,
  runInstalledExtensionTest,
  terminateExactProcesses,
  managedCoreImage,
} from "./isolated-vscode.mjs";

const target = `${process.platform}-${process.arch}`;
const source = process.argv[2];
const marketplace = source === "--marketplace";
const archive = source && !marketplace ? path.resolve(source) : null;
if (!source) {
  throw new Error("usage: node tooling/installed-package.mjs <platform.vsix|--marketplace>");
}
if (archive) {
  await access(archive);
}

const temporaryRoot = process.platform === "darwin" ? "/tmp" : os.tmpdir();
const MARKETPLACE_INSTALL_DEADLINE_MS = 15 * 60_000;
const MARKETPLACE_INSTALL_INTERVAL_MS = 15_000;
const PACKAGE_JOURNEY_DEADLINE_MS = 3 * 60_000;
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-package-"));
const resultPath = path.join(temporary, "result.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const userData = path.join(temporary, "user-data");
const extensions = path.join(temporary, "extensions");
const verifier = path.join(temporary, "verifier");
const workspace = path.join(temporary, "first-run-workspace");
const testEntry = path.join(verifier, "installedPackage.test.cjs");
let bundledCore = null;
let managedCore = null;

try {
  await Promise.all([
    mkdir(path.join(userData, "User"), { recursive: true }),
    mkdir(extensions, { recursive: true }),
    mkdir(verifier, { recursive: true }),
    mkdir(workspace, { recursive: true }),
  ]);
  await Promise.all([
    writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify(isolatedProfileSettings),
      "utf8",
    ),
    writeFile(
      path.join(verifier, "package.json"),
      JSON.stringify({
        name: "runtrol-package-verifier",
        publisher: "runtrol-tests",
        version: "0.0.0",
        engines: { vscode: "^1.100.0" },
        main: "./noop.js",
      }),
      "utf8",
    ),
    writeFile(path.join(verifier, "noop.js"), "exports.activate = () => undefined;\n", "utf8"),
  ]);
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "installedPackage.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });

  const vscode = await acquireVSCode(path.join(os.tmpdir(), "runtrol-vscode-test-cache"));
  let installedDirectory;
  if (marketplace) {
    installedDirectory = await installPublishedMarketplaceExtension(
      vscode.cli,
      userData,
      extensions,
    );
  } else {
    installVSIX(vscode.cli, archive, userData, extensions);
    installedDirectory = await findInstalledExtension(extensions, packageManifest.version);
  }
  bundledCore = path.join(
    installedDirectory,
    "resources",
    "core",
    process.platform === "win32" ? "runtrol.exe" : "runtrol",
  );
  await access(bundledCore);
  managedCore = await managedCoreImage(userData, extensionIdentifier, bundledCore);

  const environment = {
    ...runtimeState.environment,
    RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    RUNTROL_VSCODE_RESULT: resultPath,
    RUNTROL_TEST_EXTENSION_VERSION: packageManifest.version,
    RUNTROL_TEST_EXTENSION_TARGET: target,
    RUNTROL_TEST_INSTALLED_ROOT: extensions,
  };
  await within(
    runInstalledExtensionTest({
      vscodeExecutablePath: vscode.executable,
      verifierRoot: verifier,
      testEntry,
      environment,
      workspace,
      userData,
      extensions,
    }),
    PACKAGE_JOURNEY_DEADLINE_MS,
    "installed package journey",
  );

  const result = JSON.parse(await readFile(resultPath, "utf8"));
  if (typeof result.failure === "string") {
    throw new Error(
      `installed package failed after ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  await access(managedCore);
  const bundledDigest = await fileDigest(bundledCore);
  if (
    result.bundledCore !== bundledCore
    || result.extensionVersion !== packageManifest.version
    || await fileDigest(managedCore) !== bundledDigest
  ) {
    throw new Error(`installed package returned an inconsistent result: ${JSON.stringify(result)}`);
  }
  // The daemon that serves this profile's home, by the home's own locator: the generation the activation
  // started must be the build the VSIX bundles, and it must still answer. Read before the daemon is stopped.
  const generations = readGenerations(managedCore, runtimeState.environment);
  process.stdout.write(`RUNTROL_VSCODE_PACKAGE ${JSON.stringify({
    ...result,
    managedCore,
    bundledDigest,
    generations,
  })}\n`);
} finally {
  if (managedCore) {
    stopIsolatedDaemon(managedCore, runtrolHome);
  }
  await terminateExactProcesses(temporary, managedCore);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

/// `runtrol status --json` against the isolated home: every generation listed and whether it answers.
function readGenerations(executable, environment) {
  const status = spawnSync(executable, ["status", "--json"], {
    env: environment,
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
  if (status.status !== 0 || !status.stdout.trim()) {
    throw new Error(`runtrol status failed: ${status.stdout}${status.stderr}`);
  }
  return JSON.parse(status.stdout.trim());
}

function stopIsolatedDaemon(executable, home) {
  spawnSync(executable, ["panic"], {
    env: { ...process.env, RUNTROL_HOME: home },
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
}

async function installPublishedMarketplaceExtension(cli, userData, extensions) {
  const exactRelease = `${extensionIdentifier}@${packageManifest.version}`;
  const deadline = Date.now() + MARKETPLACE_INSTALL_DEADLINE_MS;
  let lastFailure = "Marketplace installation has not started";
  do {
    await rm(extensions, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    await mkdir(extensions, { recursive: true });
    try {
      installMarketplaceExtension(cli, exactRelease, userData, extensions);
      return await findInstalledExtension(extensions, packageManifest.version);
    } catch (error) {
      lastFailure = error instanceof Error ? error.message : String(error);
    }
    if (Date.now() + MARKETPLACE_INSTALL_INTERVAL_MS >= deadline) break;
    await delay(MARKETPLACE_INSTALL_INTERVAL_MS);
  } while (Date.now() < deadline);
  throw new Error(
    `Marketplace did not install ${exactRelease} within the propagation deadline: ${lastFailure}`,
  );
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function within(work, milliseconds, label) {
  let timer;
  return Promise.race([
    Promise.resolve(work),
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}
