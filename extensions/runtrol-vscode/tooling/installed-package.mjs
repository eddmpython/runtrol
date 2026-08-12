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
import { fileURLToPath } from "node:url";

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
  runInstalledExtensionTest,
  terminateExactProcesses,
} from "./isolated-vscode.mjs";

const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
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
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-package-"));
const resultPath = path.join(temporary, "result.json");
const runtrolHome = path.join(temporary, "runtrol-home");
const userData = path.join(temporary, "user-data");
const extensions = path.join(temporary, "extensions");
const verifier = path.join(temporary, "verifier");
const testEntry = path.join(verifier, "installedPackage.test.cjs");
let bundledCore = null;
let managedCore = null;

try {
  await Promise.all([
    mkdir(path.join(userData, "User"), { recursive: true }),
    mkdir(extensions, { recursive: true }),
    mkdir(verifier, { recursive: true }),
  ]);
  await Promise.all([
    writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({ "workbench.startupEditor": "none" }),
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

  const vscode = await acquireVSCode(path.join(extensionRoot, ".vscode-test"));
  if (marketplace) {
    installMarketplaceExtension(vscode.cli, extensionIdentifier, userData, extensions);
  } else {
    installVSIX(vscode.cli, archive, userData, extensions);
  }
  const installedDirectory = await findInstalledExtension(extensions);
  bundledCore = path.join(
    installedDirectory,
    "resources",
    "core",
    process.platform === "win32" ? "runtrol.exe" : "runtrol",
  );
  await access(bundledCore);
  managedCore = path.join(
    userData,
    "User",
    "globalStorage",
    extensionIdentifier,
    "core",
    process.platform === "win32" ? "runtrol.exe" : "runtrol",
  );

  await runInstalledExtensionTest({
    vscodeExecutablePath: vscode.executable,
    verifierRoot: verifier,
    testEntry,
    environment: {
      RUNTROL_HOME: runtrolHome,
      RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
      RUNTROL_VSCODE_RESULT: resultPath,
      RUNTROL_TEST_EXTENSION_VERSION: packageManifest.version,
      RUNTROL_TEST_EXTENSION_TARGET: target,
      RUNTROL_TEST_INSTALLED_ROOT: extensions,
    },
    workspace: repositoryRoot,
    userData,
    extensions,
  });

  const result = JSON.parse(await readFile(resultPath, "utf8"));
  if (typeof result.failure === "string") {
    throw new Error(
      `installed package failed after ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  await access(managedCore);
  if (
    result.bundledCore !== bundledCore
    || result.extensionVersion !== packageManifest.version
    || await fileDigest(managedCore) !== await fileDigest(bundledCore)
  ) {
    throw new Error(`installed package returned an inconsistent result: ${JSON.stringify(result)}`);
  }
  process.stdout.write(`RUNTROL_VSCODE_PACKAGE ${JSON.stringify({ ...result, managedCore })}\n`);
} finally {
  if (managedCore) {
    stopIsolatedDaemon(managedCore, runtrolHome);
  }
  await terminateExactProcesses(temporary, managedCore);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

function stopIsolatedDaemon(executable, home) {
  spawnSync(executable, ["panic"], {
    env: { ...process.env, RUNTROL_HOME: home },
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
}
