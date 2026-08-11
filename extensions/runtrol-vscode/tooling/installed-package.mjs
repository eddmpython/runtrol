import { spawnSync } from "node:child_process";
import {
  access,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const packageManifest = JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8"));
const target = `${process.platform}-${process.arch}`;
const archive = process.argv[2] ? path.resolve(process.argv[2]) : null;
if (!archive) {
  throw new Error("usage: node tooling/installed-package.mjs <platform.vsix>");
}
await access(archive);

const temporaryRoot = process.platform === "darwin" ? "/tmp" : os.tmpdir();
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-package-"));
const resultPath = path.join(temporary, "result.json");
const runtrolHome = path.join(temporary, "runtrol-home");
const userData = path.join(temporary, "user-data");
const extensions = path.join(temporary, "extensions");
const verifier = path.join(temporary, "verifier");
const testEntry = path.join(verifier, "installedPackage.test.cjs");
let bundledCore = null;

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

  const vscodeExecutablePath = await downloadAndUnzipVSCode({
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || "stable",
    cachePath: path.join(extensionRoot, ".vscode-test"),
  });
  const [cli] = resolveCliArgsFromVSCodeExecutablePath(vscodeExecutablePath);
  const installed = spawnSync(
    cli,
    [
      "--user-data-dir",
      userData,
      "--extensions-dir",
      extensions,
      "--install-extension",
      archive,
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
      `VSIX installation failed: ${installed.error?.message ?? `exit ${String(installed.status)}`}\n`
      + `${installed.stdout ?? ""}${installed.stderr ?? ""}`,
    );
  }
  const installedDirectory = await findInstalledExtension(extensions);
  bundledCore = path.join(
    installedDirectory,
    "resources",
    "core",
    process.platform === "win32" ? "runtrol.exe" : "runtrol",
  );
  await access(bundledCore);

  await runTests({
    vscodeExecutablePath,
    extensionDevelopmentPath: verifier,
    extensionTestsPath: testEntry,
    extensionTestsEnv: {
      RUNTROL_HOME: runtrolHome,
      RUNTROL_VSCODE_RESULT: resultPath,
      RUNTROL_TEST_EXTENSION_VERSION: packageManifest.version,
      RUNTROL_TEST_EXTENSION_TARGET: target,
      RUNTROL_TEST_INSTALLED_ROOT: extensions,
    },
    launchArgs: [
      repositoryRoot,
      "--disable-workspace-trust",
      "--skip-welcome",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensions}`,
    ],
  });

  const result = JSON.parse(await readFile(resultPath, "utf8"));
  if (typeof result.failure === "string") {
    throw new Error(
      `installed package failed after ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  if (result.corePath !== bundledCore || result.extensionVersion !== packageManifest.version) {
    throw new Error(`installed package returned an inconsistent result: ${JSON.stringify(result)}`);
  }
  process.stdout.write(`RUNTROL_VSCODE_PACKAGE ${JSON.stringify(result)}\n`);
} finally {
  if (bundledCore) {
    stopIsolatedDaemon(bundledCore, runtrolHome);
  }
  await terminateExactProcesses(temporary, bundledCore);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

async function findInstalledExtension(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const matches = entries.filter(
    (entry) => entry.isDirectory() && entry.name.startsWith("eddmpython.runtrol-studio-"),
  );
  if (matches.length !== 1) {
    throw new Error(`expected one isolated Runtrol Studio installation, found ${matches.length}`);
  }
  return path.join(root, matches[0].name);
}

function stopIsolatedDaemon(executable, home) {
  spawnSync(executable, ["panic"], {
    env: { ...process.env, RUNTROL_HOME: home },
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
}

async function terminateExactProcesses(marker, executable) {
  const pids = process.platform === "win32"
    ? windowsPids(marker, executable)
    : unixPids(marker, executable);
  for (const pid of pids) {
    if (pid === process.pid) continue;
    try {
      process.kill(pid, "SIGTERM");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  if (pids.length > 0) {
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

function windowsPids(marker, executable) {
  const query = "$marker=[Environment]::GetEnvironmentVariable('RUNTROL_CLEANUP_MARKER'); "
    + "$core=[Environment]::GetEnvironmentVariable('RUNTROL_CLEANUP_CORE'); "
    + "Get-CimInstance Win32_Process | Where-Object { "
    + "($_.CommandLine -and $_.CommandLine.Contains($marker)) -or "
    + "($core -and $_.ExecutablePath -eq $core) "
    + "} | Select-Object -ExpandProperty ProcessId";
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", query],
    {
      env: {
        ...process.env,
        RUNTROL_CLEANUP_MARKER: marker,
        RUNTROL_CLEANUP_CORE: executable || "",
      },
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    },
  );
  if (result.status !== 0) {
    throw new Error(`cannot enumerate isolated Windows processes: ${result.stderr}`);
  }
  return result.stdout.split(/\s+/).map(Number).filter((pid) => Number.isInteger(pid) && pid > 0);
}

function unixPids(marker, executable) {
  const result = spawnSync("ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
    timeout: 15_000,
  });
  if (result.status !== 0) {
    throw new Error(`cannot enumerate isolated Unix processes: ${result.stderr}`);
  }
  return result.stdout.split("\n").flatMap((line) => {
    const match = /^\s*(\d+)\s+(.*)$/.exec(line);
    if (!match) return [];
    const command = match[2];
    return command.includes(marker) || (executable && command.includes(executable))
      ? [Number(match[1])]
      : [];
  });
}
