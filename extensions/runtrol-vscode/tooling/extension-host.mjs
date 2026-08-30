import { spawn, spawnSync } from "node:child_process";
import { openSync, readFileSync } from "node:fs";
import { cp, mkdtemp, mkdir, readdir, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
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
  ownedTreeIdentities,
  quietExtensionTestArguments,
  terminateCapturedIdentities,
  TESTED_VSCODE_VERSION,
} from "./isolated-vscode.mjs";

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
  env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);
}

// macOS expands its per-user temporary directory to a path long enough to exceed the Unix-domain socket
// ceiling once runtrol's home and socket names are appended. `/tmp` is the kernel-stable short alias for
// exactly this purpose, and the random suffix still isolates concurrent runs.
// Canonicalize the temporary root before deriving any workspace names. Core canonicalizes approved roots and
// provider-owned conversation paths at its security boundary; leaving only the harness on an alias such as
// macOS `/tmp` or a Windows 8.3 name would falsely report that a conversation in the same folder never arrived.
const temporaryRoot = await realpath(process.platform === "darwin" ? "/tmp" : os.tmpdir());
const temporary = await mkdtemp(path.join(temporaryRoot, "rvh-"));
// A crashed earlier run leaves its whole isolated world behind, and those leftovers accumulate forever and even
// surface as windows full of fake workspaces during later tests. Every run therefore starts by deleting its
// stale predecessors. The age guard keeps a concurrent run's live directory safe; only abandoned ones go.
const STALE_RUN_AGE_MS = 2 * 60 * 60 * 1000;
for (const entry of await readdir(temporaryRoot).catch(() => [])) {
  if (!entry.startsWith("rvh-")) continue;
  const stale = path.join(temporaryRoot, entry);
  if (stale === temporary) continue;
  const age = await stat(stale).then((s) => Date.now() - s.mtimeMs).catch(() => 0);
  if (age < STALE_RUN_AGE_MS) continue;
  // ok: a locked leftover must not fail this run. The run proceeds on its own fresh directory either way, and
  // the next run sweeps the leftover again.
  await rm(stale, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 }).catch(() => {});
}
const output = path.join(temporary, "tests");
const testEntry = path.join(output, "extensionHost.test.cjs");
const extensionUnderTestRoot = path.join(temporary, "extension");
const resultPath = path.join(temporary, "result.json");
const restoreResultPath = path.join(temporary, "restore-result.json");
const followResultPath = path.join(temporary, "follow-result.json");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const userData = path.join(temporary, "user");
const measureExtensions = path.join(temporary, "extensions-measure");
const restoreExtensions = path.join(temporary, "extensions-restore");
const followExtensions = path.join(temporary, "extensions-follow");
// The follow phase's two folders live outside the measured workspaces so no earlier phase can have granted
// them: the second folder's conversation can then only arrive through the live follow chain under test.
const followRoot = path.join(temporary, "follow");
const followFirst = path.join(followRoot, "alpha");
const followTarget = path.join(followRoot, "beta");
// A saved workspace file rather than a plain folder: adding a folder to a workspace-file window keeps the same
// extension host alive, and a live host is the entire point of the phase.
// Keep the workspace file outside both fixture folders and their parent. VS Code treats the file's own
// directory as window context, so placing it above alpha and beta would grant beta before it is opened and
// make the live root-following proof invalid.
const followWorkspaceFile = path.join(temporary, "follow.code-workspace");
const workspaceRoot = path.join(temporary, "workspaces");
const workspaces = Array.from(
  { length: 30 },
  (_unused, index) => path.join(workspaceRoot, `workspace-${index + 1}`),
);
const providers = path.join(runtrolHome, "providers");
const pathKey = Object.keys(process.env).find((name) => name.toLowerCase() === "path") ?? "PATH";
const coreEnvironment = runtimeState.environment;
coreEnvironment.RUNTROL_ACP_FIXTURE_UNIQUE_SESSIONS = "1";
// The daemon's close path narrates its steps on stderr under this switch (`serve.rs`), and this harness is the
// only reader of that stderr: a close that exceeds the CLI timeout is then diagnosed from the last step named
// instead of from the two words ETIMEDOUT carries.
coreEnvironment.RUNTROL_CLOSE_TRACE = "1";
// The performance contract measures the declared fixture and its 30 sessions. Inheriting the operator's PATH
// also discovers and probes every installed coding CLI while the clock runs, so an account probe or a cold CLI
// filesystem walk becomes a random refresh result. The exact fixture directory is the complete provider surface
// for this isolated home; Code, Core, and the fixture itself are already launched by absolute path.
coreEnvironment[pathKey] = path.dirname(fixture);
let daemon = null;
const daemonStderrPath = path.join(temporary, "core-stderr.log");
// Everything the daemon has written so far, read back from its spool.
function daemonStderrText() {
  try {
    return readFileSync(daemonStderrPath, "utf8");
  } catch {
    // ok: a daemon that never started wrote nothing, and the caller's own message says what failed.
    return "";
  }
}
const managedSessions = [];
// Core spawns one provider fixture per session as its own child. Those children outlive a kill of
// the daemon root, so the tree is captured while it is alive and terminated from the snapshot.
const ownedProcesses = new Map();
const automaticWindowArguments = process.env.RUNTROL_VSCODE_CAPTURE
  ? []
  : quietExtensionTestArguments;

try {
  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  await mkdir(extensionUnderTestRoot, { recursive: true });
  await Promise.all([
    cp(path.join(extensionRoot, "package.json"), path.join(extensionUnderTestRoot, "package.json")),
    cp(path.join(extensionRoot, "dist"), path.join(extensionUnderTestRoot, "dist"), { recursive: true }),
    cp(path.join(extensionRoot, "resources"), path.join(extensionUnderTestRoot, "resources"), { recursive: true }),
  ]);
  await mkdir(path.join(userData, "User"), { recursive: true });
  await mkdir(measureExtensions, { recursive: true });
  await mkdir(restoreExtensions, { recursive: true });
  await mkdir(followExtensions, { recursive: true });
  await mkdir(followFirst, { recursive: true });
  await mkdir(followTarget, { recursive: true });
  await writeFile(
    followWorkspaceFile,
    JSON.stringify({ folders: [{ path: followFirst }] }),
    "utf8",
  );
  for (const workspace of workspaces) {
    await mkdir(workspace, { recursive: true });
  }
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
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "runtrol.followWorkspace": false,
    }),
    "utf8",
  );
  // The daemon's stderr goes to a file, never a pipe: this harness waits on CLI commands with spawnSync,
  // during which nothing drains a pipe, and a daemon whose diagnostics fill 64 KiB then blocks on its next
  // write and can answer nobody (measured 2026-08-28 on the Linux host: the heartbeat stopped mid-run and
  // the stall handler could not print either, because the thread was inside the blocked write).
  const daemonStderrFd = openSync(daemonStderrPath, "a");
  daemon = spawn(core, ["daemon"], {
    env: coreEnvironment,
    stdio: ["ignore", "ignore", daemonStderrFd],
    windowsHide: true,
  });
  await delay(500);
  if (daemon.exitCode !== null) {
    throw new Error(`test Core stopped during startup:\n${daemonStderrText()}`);
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
  for (const workspace of workspaces) startManagedSession(workspace);
  captureOwnedProcesses();
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
    await Promise.all([
      runHost(
        installed,
        testEntry,
        resultPath,
        testEnvironment,
        workspaceRoot,
        measureExtensions,
      ),
    ]);
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
  const restoreHost = runHost(
    installed,
    testEntry,
    restoreResultPath,
    restoreEnvironment,
    restoreWorkspace,
    restoreExtensions,
  );
  await restoreHost;
  const restored = JSON.parse(await readFile(restoreResultPath, "utf8"));
  // These conversations are deliberately outside the measured window's approved root. The follow window first
  // opens alpha and then adds beta, proving each real workspace event widens discovery. Without real sessions in
  // both folders the phase waits for fixtures that do not exist and cannot test the product chain at all.
  for (const workspace of [followFirst, followTarget]) startManagedSession(workspace);
  const followEnvironment = {
    ...testEnvironment,
    RUNTROL_VSCODE_RESULT: followResultPath,
    RUNTROL_VSCODE_PHASE: "follow",
    RUNTROL_VSCODE_FOLLOW_TARGET: followTarget,
  };
  await Promise.all([
    runHost(
      installed,
      testEntry,
      followResultPath,
      followEnvironment,
      followWorkspaceFile,
      followExtensions,
    ),
    captureWhenReady(followResultPath),
  ]);
  const followed = JSON.parse(await readFile(followResultPath, "utf8"));
  const result = { ...measured, ...restored, ...followed };
  process.stdout.write(`RUNTROL_VSCODE_HOST ${JSON.stringify(result)}\n`);
} catch (error) {
  let hostError = error;
  const reported = await readFile(resultPath, "utf8")
    .then((contents) => JSON.parse(contents))
    .catch(() => null);
  if (reported && typeof reported.failure === "string") {
    hostError = new Error(
      `the VS Code test failed at ${String(reported.stage || "unknown")}: ${reported.failure}`
      + (typeof reported.stack === "string" ? `\n${reported.stack}` : ""),
      { cause: error },
    );
  }
  if (daemon && daemon.exitCode !== null) {
    throw new Error(`the VS Code host run failed after Core exited with ${String(daemon?.exitCode)}`, {
      cause: hostError,
    });
  }
  const crash = await readFile(path.join(runtrolHome, "daemon-crash.log"), "utf8").catch(
    (readError) => readError.code === "ENOENT" ? "" : Promise.reject(readError),
  );
  if (crash) {
    throw new Error(`the VS Code host run failed after a Core crash:\n${crash}`, { cause: hostError });
  }
  if (daemonStderrText()) {
    throw new Error(`the VS Code host run failed and Core reported:\n${daemonStderrText()}`, { cause: hostError });
  }
  throw hostError;
} finally {
  const cleanupFailures = [];
  // The test may open further sessions from inside VS Code, so the tree is captured once more
  // while Core is still alive and can still be descended.
  captureOwnedProcesses();
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
      // Say which failure this was: a refusal carries output, a hang carries only the timeout error, and a
      // signal death carries the signal. All three used to read as the same two words.
      const why = closed.stderr || closed.stdout
        || (closed.error ? String(closed.error.code ?? closed.error.message) : "")
        || `exit ${String(closed.status)} signal ${String(closed.signal)}`;
      cleanupFailures.push(`session ${session}: close failed (${why.trim()})`);
    }
  }
  // The exact tree snapshot includes Core itself. End the whole owned tree in one convergent sweep instead
  // of waiting on ChildProcess.close, which can remain pending while a descendant still owns an inherited
  // stream even after the root process has exited.
  try {
    await terminateCapturedIdentities([...ownedProcesses.values()]);
  } catch (error) {
    cleanupFailures.push(error.message);
  }
  await rm(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  if (cleanupFailures.length > 0) {
    // The daemon's own words go with the failure: a close that times out is diagnosed from what the Core said
    // while it was closing, and nothing else in this harness sees that stream.
    throw new Error(`hot-session cleanup failed:\n${cleanupFailures.join("\n")}\nCore stderr:\n${daemonStderrText()}`);
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

// Snapshots the Core tree so cleanup can reach fixtures that Core spawned. Enumerating processes
// costs a PowerShell round trip, so this runs only outside the measured window.
function captureOwnedProcesses() {
  if (!daemon || daemon.exitCode !== null) {
    return;
  }
  for (const identity of ownedTreeIdentities(daemon.pid)) {
    ownedProcesses.set(identity.pid, identity);
  }
}

function startManagedSession(workspace) {
  const started = spawnSync(core, ["start", "fixture-acp", workspace], {
    env: coreEnvironment,
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
  const session = started.stdout.trim();
  if (started.status !== 0 || !session) {
    // A start that hangs to the CLI timeout has empty output; what the daemon said meanwhile is the only
    // evidence of where it stopped answering (measured 2026-08-27 on the Linux host harness).
    const why = started.error ? String(started.error.code ?? started.error.message) : "";
    throw new Error(
      `cannot start a hot ACP fixture session: ${why}\n${started.stdout}${started.stderr}\nCore stderr:\n${daemonStderrText()}`,
    );
  }
  managedSessions.push(session);
}

// The eye-pass photographer. Only does anything when RUNTROL_VSCODE_CAPTURE names an output file: it waits for
// the follow phase to say the panel is posed, photographs the follow window from outside the extension host,
// and hands the phase its confirmation file so it can finish.
async function captureWhenReady(followResultPath) {
  const outPath = process.env.RUNTROL_VSCODE_CAPTURE;
  if (!outPath) return;
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const progress = await readFile(followResultPath, "utf8")
      .then((contents) => JSON.parse(contents))
      // ok: the phase has simply not written its first checkpoint yet; the loop asks again until the deadline.
      .catch(() => null);
    if (progress && typeof progress.failure === "string") return;
    if (progress && (typeof progress.vscode === "string" || progress.stage === "capture-ready")) {
      if (progress.stage === "capture-ready") {
        const shot = spawnSync(
          "powershell.exe",
          [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            path.join(extensionRoot, "tooling", "capture-window.ps1"),
            "-TitleMatch",
            "follow (Workspace)",
            "-OutPath",
            outPath,
          ],
          { encoding: "utf8", timeout: 30_000, windowsHide: true },
        );
        if (shot.status !== 0) {
          throw new Error(`the eye-pass capture failed:\n${shot.stdout}${shot.stderr}`);
        }
        await writeFile(`${followResultPath}.captured`, "1", "utf8");
      }
      return;
    }
    await delay(300);
  }
  throw new Error("the follow phase never reached its capture pose");
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
    cachePath: path.join(os.tmpdir(), "runtrol-vscode-test-cache"),
    extensionDevelopmentPath: extensionUnderTestRoot,
    extensionTestsPath: testEntry,
    extensionTestsEnv: testEnvironment,
    launchArgs: [
      workspace,
      ...isolatedLaunchArguments,
      ...automaticWindowArguments,
      "--disable-extensions",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensionsDirectory}`,
    ],
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || TESTED_VSCODE_VERSION,
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
    ...isolatedLaunchArguments,
    ...automaticWindowArguments,
    "--disable-extensions",
    "--disable-workspace-trust",
    "--skip-welcome",
    "--user-data-dir",
    userData,
    "--extensions-dir",
    extensionsDirectory,
    "--extensionDevelopmentPath",
    extensionUnderTestRoot,
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
