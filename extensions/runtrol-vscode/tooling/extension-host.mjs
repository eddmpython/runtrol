import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runTests } from "@vscode/test-electron";
import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";

const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const core = process.env.RUNTROL_TEST_CORE
  ? path.resolve(process.env.RUNTROL_TEST_CORE)
  : path.join(repositoryRoot, "target", "debug", process.platform === "win32" ? "runtrol.exe" : "runtrol");
await stat(core);
const fixtureSetting = process.env.RUNTROL_TEST_ACP_FIXTURE;
if (!fixtureSetting) {
  throw new Error("RUNTROL_TEST_ACP_FIXTURE is required");
}
const fixture = path.resolve(fixtureSetting);
await stat(fixture);

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`production extension build failed:\n${bundled.stdout}${bundled.stderr}`);
}

// macOS expands its per-user temporary directory to a path long enough to exceed the Unix-domain socket
// ceiling once runtrol's home and socket names are appended. `/tmp` is the kernel-stable short alias for
// exactly this purpose, and the random suffix still isolates concurrent runs.
const temporaryRoot = process.platform === "darwin" ? "/tmp" : os.tmpdir();
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-host-"));
const output = path.join(extensionRoot, ".test-dist");
const testEntry = path.join(output, "extensionHost.test.cjs");
const resultPath = path.join(temporary, "result.json");
const restoreResultPath = path.join(temporary, "restore-result.json");
const runtrolHome = path.join(temporary, "runtrol-home");
const userData = path.join(temporary, "user");
const measureExtensions = path.join(temporary, "extensions-measure");
const restoreExtensions = path.join(temporary, "extensions-restore");
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await mkdir(path.join(userData, "User"), { recursive: true });
await mkdir(measureExtensions, { recursive: true });
await mkdir(restoreExtensions, { recursive: true });
const workspaces = Array.from({ length: 30 }, (_unused, index) => path.join(temporary, `workspace-${index + 1}`));
for (const workspace of workspaces) {
  await mkdir(workspace, { recursive: true });
}
const providers = path.join(runtrolHome, "providers");
await mkdir(providers, { recursive: true });
await writeFile(
  path.join(providers, "fixture-acp.toml"),
  `schema = 1
id = "fixture-acp"
display_name = "ACP Fixture"
kind = "acp"

[bin]
names = ["${path.basename(fixture)}"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
`,
  "utf8",
);
await writeFile(
  path.join(userData, "User", "settings.json"),
  JSON.stringify({
    "runtrol.corePath": core,
    "runtrol.followWorkspace": false,
    "workbench.startupEditor": "none",
  }),
  "utf8",
);
const pathKey = Object.keys(process.env).find((name) => name.toLowerCase() === "path") ?? "PATH";
const coreEnvironment = { ...process.env, RUNTROL_HOME: runtrolHome };
coreEnvironment.RUNTROL_ACP_FIXTURE_UNIQUE_SESSIONS = "1";
coreEnvironment[pathKey] = `${path.dirname(fixture)}${path.delimiter}${process.env[pathKey] ?? ""}`;
let daemon = null;
let daemonStderr = "";
const managedSessions = [];

try {
  daemon = spawn(core, ["daemon"], {
    env: coreEnvironment,
    stdio: ["ignore", "ignore", "pipe"],
    windowsHide: true,
  });
  daemon.stderr.setEncoding("utf8").on("data", (chunk) => {
    daemonStderr += chunk;
  });
  await delay(500);
  if (daemon.exitCode !== null) {
    throw new Error(`test Core stopped during startup:\n${daemonStderr}`);
  }
  const reached = spawnSync(core, ["endpoint"], {
    env: coreEnvironment,
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
  if (reached.status !== 0 || !reached.stdout.trim()) {
    throw new Error(`test Core did not expose an endpoint:\n${reached.stdout}${reached.stderr}`);
  }
  for (const workspace of workspaces) {
    const started = spawnSync(core, ["start", "fixture-acp", workspace], {
      env: coreEnvironment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
    const session = started.stdout.trim();
    if (started.status !== 0 || !session) {
      throw new Error(`cannot start a hot ACP fixture session:\n${started.stdout}${started.stderr}`);
    }
    managedSessions.push(session);
  }
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "extensionHost.test.ts")],
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
    ...coreEnvironment,
    RUNTROL_TEST_CORE: core,
    RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    RUNTROL_VSCODE_PERFORMANCE: "1",
    RUNTROL_VSCODE_RESULT: resultPath,
    RUNTROL_VSCODE_PHASE: "measure",
    RUNTROL_VSCODE_MANAGED_SESSIONS: JSON.stringify(managedSessions),
  };
  const installed = process.env.RUNTROL_TEST_VSCODE_EXECUTABLE;
  let resumedAdopted = false;
  try {
    await runHost(
      installed,
      testEntry,
      resultPath,
      testEnvironment,
      repositoryRoot,
      measureExtensions,
    );
  } finally {
    const progress = await readFile(resultPath, "utf8")
      .then((contents) => JSON.parse(contents))
      .catch(() => null);
    resumedAdopted = adoptResumedSession(managedSessions, progress);
  }
  const measured = JSON.parse(await readFile(resultPath, "utf8"));
  if (!resumedAdopted) {
    throw new Error("the host measurement did not identify the cold session it resumed");
  }
  const restoreSession = measured.restoreSession;
  const restoreWorkspace = measured.restoreWorkspace;
  if (typeof restoreSession !== "string" || !restoreSession || typeof restoreWorkspace !== "string" || !restoreWorkspace) {
    throw new Error("the host measurement did not identify its final selected session");
  }
  const restoreEnvironment = {
    ...testEnvironment,
    RUNTROL_VSCODE_RESULT: restoreResultPath,
    RUNTROL_VSCODE_PHASE: "restore",
    RUNTROL_VSCODE_RESTORE_SESSION: restoreSession,
  };
  await runHost(
    installed,
    testEntry,
    restoreResultPath,
    restoreEnvironment,
    restoreWorkspace,
    restoreExtensions,
  );
  const restored = JSON.parse(await readFile(restoreResultPath, "utf8"));
  const result = { ...measured, ...restored };
  process.stdout.write(`RUNTROL_VSCODE_HOST ${JSON.stringify(result)}\n`);
} catch (error) {
  if (daemon?.exitCode !== null) {
    throw new Error(`the VS Code host run failed after Core exited with ${String(daemon?.exitCode)}`, {
      cause: error,
    });
  }
  const crash = await readFile(path.join(runtrolHome, "daemon-crash.log"), "utf8").catch(
    (readError) => readError.code === "ENOENT" ? "" : Promise.reject(readError),
  );
  if (crash) {
    throw new Error(`the VS Code host run failed after a Core crash:\n${crash}`, { cause: error });
  }
  if (daemonStderr) {
    throw new Error(`the VS Code host run failed and Core reported:\n${daemonStderr}`, { cause: error });
  }
  throw error;
} finally {
  const cleanupFailures = [];
  for (const session of [...managedSessions].reverse()) {
    if (daemon?.exitCode !== null) {
      cleanupFailures.push(`Core exited before session ${session} could close`);
      break;
    }
    const closed = spawnSync(core, ["close", session, "--now"], {
      env: coreEnvironment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
    if (closed.status !== 0) {
      cleanupFailures.push(`session ${session}: ${closed.stderr || closed.stdout || "close failed"}`);
    }
  }
  if (daemon?.exitCode === null) {
    const exited = new Promise((resolve) => daemon.once("close", resolve));
    daemon.kill();
    await Promise.race([
      exited,
      delay(5_000).then(() => Promise.reject(new Error("test Core did not terminate within 5 seconds"))),
    ]);
  }
  await rm(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  if (cleanupFailures.length > 0) {
    throw new Error(`hot-session cleanup failed:\n${cleanupFailures.join("\n")}`);
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function adoptResumedSession(sessions, result) {
  if (
    !result
    || typeof result.resumedFrom !== "string"
    || typeof result.resumedTo !== "string"
    || !result.resumedTo
  ) {
    return false;
  }
  const resumedIndex = sessions.indexOf(result.resumedFrom);
  if (resumedIndex < 0) {
    return false;
  }
  sessions[resumedIndex] = result.resumedTo;
  return true;
}

async function runHost(installed, testEntry, resultPath, testEnvironment, workspace, extensionsDirectory) {
  if (installed?.toLowerCase().endsWith(".cmd")) {
    await runInstalledCode(
      installed,
      testEntry,
      resultPath,
      testEnvironment,
      workspace,
      extensionsDirectory,
    );
    return;
  }
  await runTests({
    cachePath: path.join(extensionRoot, ".vscode-test"),
    extensionDevelopmentPath: extensionRoot,
    extensionTestsPath: testEntry,
    extensionTestsEnv: testEnvironment,
    launchArgs: [
      workspace,
      "--disable-extensions",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensionsDirectory}`,
    ],
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || "stable",
    vscodeExecutablePath: installed || undefined,
  });
}

async function runInstalledCode(
  executable,
  testEntry,
  resultPath,
  testEnvironment,
  workspace,
  extensionsDirectory,
) {
  const arguments_ = [
    "--new-window",
    "--disable-extensions",
    "--disable-workspace-trust",
    "--skip-welcome",
    "--user-data-dir",
    userData,
    "--extensions-dir",
    extensionsDirectory,
    "--extensionDevelopmentPath",
    extensionRoot,
    "--extensionTestsPath",
    testEntry,
    workspace,
  ];
  let child;
  const started = new Promise((resolve, reject) => {
    child = spawn(`"${executable}"`, arguments_, {
      env: { ...process.env, ...testEnvironment },
      shell: true,
      stdio: "inherit",
      windowsHide: true,
    });
    child.once("error", reject);
    child.once("spawn", resolve);
  });
  await started;

  try {
    const deadline = Date.now() + 30_000;
    let lastStage = "not started";
    while (Date.now() < deadline) {
      try {
        const result = JSON.parse(await readFile(resultPath, "utf8"));
        if (typeof result.vscode === "string") {
          return;
        }
        if (typeof result.failure === "string") {
          throw new Error(
            `installed VS Code test failed after checkpoint ${String(result.stage || lastStage)}: ${result.failure}`
            + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
          );
        }
        if (typeof result.stage === "string") {
          lastStage = result.stage;
        }
      } catch (error) {
        if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) {
          throw error;
        }
      }
      await delay(100);
    }
    throw new Error(`installed VS Code test timed out after checkpoint ${lastStage}`);
  } finally {
    await terminateInstalledCode(userData);
    if (child?.exitCode === null) {
      child.kill();
    }
  }
}

async function terminateInstalledCode(marker) {
  if (process.platform !== "win32") {
    return;
  }
  const query = "$marker=[Environment]::GetEnvironmentVariable('RUNTROL_VSCODE_MARKER'); "
    + "Get-CimInstance Win32_Process -Filter \"Name = 'Code.exe'\" "
    + "| Where-Object { $_.CommandLine -and $_.CommandLine.Contains($marker) } "
    + "| Select-Object -ExpandProperty ProcessId";
  const listed = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", query],
    {
      env: { ...process.env, RUNTROL_VSCODE_MARKER: marker },
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
    },
  );
  if (listed.status !== 0) {
    throw new Error(`cannot enumerate the isolated VS Code processes: ${listed.stderr}`);
  }
  const pids = listed.stdout.split(/\s+/).map(Number).filter(
    (pid) => Number.isInteger(pid) && pid > 0 && pid !== process.pid,
  );
  for (const pid of pids) {
    try {
      process.kill(pid, "SIGTERM");
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  if (pids.length > 0) {
    await delay(500);
  }
}
