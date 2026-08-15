import { spawnSync } from "node:child_process";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import { approveNextTestIntegration } from "./integration-approval.mjs";
import {
  acquireVSCode,
  fileDigest,
  findInstalledExtension,
  installVSIX,
  isolatedProfileSettings,
  isolatedRuntimeState,
  runInstalledExtensionTest,
  terminateExactProcesses,
  uninstallExtension,
} from "./isolated-vscode.mjs";
import { descendantPids, normalizedExecutable, processRows } from "./process-identity.mjs";

const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const [baselineArchiveArgument, currentArchiveArgument, baselineVersion, currentVersion, fixtureArgument] =
  process.argv.slice(2);
if (!baselineArchiveArgument || !currentArchiveArgument || !baselineVersion || !currentVersion || !fixtureArgument) {
  throw new Error(
    "usage: node tooling/upgrade-rollback.mjs <baseline.vsix> <current.vsix> "
    + "<baseline-version> <current-version> <acp-fixture>",
  );
}
const baselineArchive = path.resolve(baselineArchiveArgument);
const currentArchive = path.resolve(currentArchiveArgument);
const fixture = path.resolve(fixtureArgument);
await Promise.all([access(baselineArchive), access(currentArchive), access(fixture)]);

const temporaryRoot = process.platform === "darwin" ? "/tmp" : os.tmpdir();
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-upgrade-"));
const userData = path.join(temporary, "user-data");
const extensions = path.join(temporary, "extensions");
const verifier = path.join(temporary, "verifier");
const runtimeState = isolatedRuntimeState(temporary);
const runtrolHome = runtimeState.home;
const workspace = path.join(temporary, "workspace");
const resultPath = path.join(temporary, "phase-result.json");
const integrationApproval = path.join(temporary, "integration-approved");
const testEntry = path.join(verifier, "upgradeRollback.test.cjs");
const managedCore = path.join(
  userData,
  "User",
  "globalStorage",
  extensionIdentifier,
  "core",
  process.platform === "win32" ? "runtrol.exe" : "runtrol",
);
const selectionFile = path.join(
  userData,
  "User",
  "globalStorage",
  extensionIdentifier,
  "selected-session.json",
);
const pathKey = Object.keys(process.env).find((name) => name.toLowerCase() === "path") ?? "PATH";
const environment = runtimeState.environment;
environment[pathKey] = `${path.dirname(fixture)}${path.delimiter}${process.env[pathKey] ?? ""}`;
let session = null;
let daemonPid = null;
let providerPid = null;

try {
  await prepareVerifier();
  await writeConfiguration();
  const vscode = await acquireVSCode(path.join(os.tmpdir(), "runtrol-vscode-test-cache"));

  installVSIX(vscode.cli, baselineArchive, userData, extensions);
  const baselineDirectory = await findInstalledExtension(extensions, baselineVersion);
  const baselineBundledCore = bundledCoreAt(baselineDirectory);
  await runPhase(vscode.executable, "bootstrap", baselineVersion, null);
  await access(managedCore);
  const baselineDigest = await fileDigest(managedCore);
  if (baselineDigest !== await fileDigest(baselineBundledCore)) {
    throw new Error("the baseline activation did not materialize its exact bundled Core");
  }

  daemonPid = await exactDaemonPid();
  session = runCore(["start", "fixture-acp", workspace]);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u.test(session)) {
    throw new Error(`Core returned an invalid session identifier: ${session}`);
  }
  await mkdir(path.dirname(selectionFile), { recursive: true });
  await writeFile(selectionFile, JSON.stringify({ schema: 1, session }), { encoding: "utf8", mode: 0o600 });
  providerPid = await exactProviderPid(daemonPid);
  await runPhase(vscode.executable, "baseline", baselineVersion, session);
  await requireSameProcesses("baseline");
  runCore(["say", session, "baseline continuity"]);

  installVSIX(vscode.cli, currentArchive, userData, extensions);
  const currentDirectory = await findInstalledExtension(extensions, currentVersion);
  if (path.resolve(currentDirectory) === path.resolve(baselineDirectory)) {
    throw new Error("the upgrade reused the baseline extension directory");
  }
  const currentBundledCore = bundledCoreAt(currentDirectory);
  await runPhase(vscode.executable, "upgrade", currentVersion, session);
  const upgradeDigest = await fileDigest(managedCore);
  if (upgradeDigest !== await fileDigest(currentBundledCore) || upgradeDigest === baselineDigest) {
    throw new Error("the upgrade did not atomically install a distinct current Core image");
  }
  await requireSameProcesses("upgrade");
  runCore(["say", session, "upgrade continuity"]);

  uninstallExtension(vscode.cli, extensionIdentifier, userData, extensions);
  installVSIX(vscode.cli, baselineArchive, userData, extensions);
  const rollbackDirectory = await findInstalledExtension(extensions, baselineVersion);
  await runPhase(vscode.executable, "rollback", baselineVersion, session);
  const rollbackDigest = await fileDigest(managedCore);
  if (rollbackDigest !== baselineDigest) {
    throw new Error("the rollback did not restore the exact baseline Core image");
  }
  await requireSameProcesses("rollback");
  runCore(["say", session, "rollback continuity"]);

  process.stdout.write(`RUNTROL_VSCODE_UPGRADE ${JSON.stringify({
    baselineVersion,
    currentVersion,
    session,
    daemonPid,
    providerPid,
    baselineDigest,
    upgradeDigest,
    rollbackDigest,
    workspace,
    baselineDirectory,
    currentDirectory,
    rollbackDirectory,
  })}\n`);
} finally {
  const expectedPids = [daemonPid, providerPid].filter((pid) => Number.isInteger(pid));
  if (session) {
    const closed = spawnSync(managedCore, ["close", session, "--now"], {
      env: environment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
    if (closed.status !== 0) {
      process.stderr.write(`upgrade rehearsal session cleanup failed: ${closed.stderr || closed.stdout}\n`);
    }
  }
  if (await exists(managedCore)) {
    spawnSync(managedCore, ["panic"], {
      env: environment,
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
  }
  await terminateExactProcesses(temporary, managedCore);
  const survivors = processRows().filter((row) => expectedPids.includes(row.pid)).map((row) => row.pid);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  if (survivors.length > 0) {
    throw new Error(`isolated upgrade rehearsal left process ${survivors.join(", ")} alive`);
  }
}

async function prepareVerifier() {
  await Promise.all([
    mkdir(path.join(userData, "User"), { recursive: true }),
    mkdir(extensions, { recursive: true }),
    mkdir(verifier, { recursive: true }),
    mkdir(workspace, { recursive: true }),
  ]);
  await Promise.all([
    writeFile(
      path.join(verifier, "package.json"),
      JSON.stringify({
        name: "runtrol-upgrade-verifier",
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
    entryPoints: [path.join(extensionRoot, "src", "integration", "upgradeRollback.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });
}

async function writeConfiguration() {
  const providers = path.join(runtrolHome, "providers");
  await mkdir(providers, { recursive: true });
  await Promise.all([
    writeFile(
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
    ),
    writeFile(
      path.join(userData, "User", "settings.json"),
      JSON.stringify({
        ...isolatedProfileSettings,
        "runtrol.followWorkspace": false,
      }),
      "utf8",
    ),
  ]);
}

async function runPhase(vscodeExecutablePath, phase, version, selectedSession) {
  await rm(resultPath, { force: true });
  // Isolated Extension Hosts may not share native SecretStorage even when they reuse the same profile. A fresh
  // marker and live IPC approval let each real pending enrollment finish without accepting a stale earlier decision.
  await rm(integrationApproval, { force: true });
  const approval = new AbortController();
  const host = runInstalledExtensionTest({
    vscodeExecutablePath,
    verifierRoot: verifier,
    testEntry,
    environment: {
      ...environment,
      RUNTROL_TEST_EXTERNAL_INTEGRATION_APPROVAL: integrationApproval,
      RUNTROL_TEST_INSTALLED_UPGRADE: "1",
      RUNTROL_VSCODE_PERFORMANCE: "1",
      RUNTROL_VSCODE_RESULT: resultPath,
      RUNTROL_VSCODE_PHASE: phase,
      RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
      RUNTROL_TEST_EXTENSION_VERSION: version,
      RUNTROL_TEST_WORKSPACE: workspace,
      ...(selectedSession ? { RUNTROL_TEST_SESSION: selectedSession } : {}),
    },
    workspace,
    userData,
    extensions,
  }).finally(() => approval.abort());
  await Promise.all([
    host,
    approveNextTestIntegration(
      managedCore,
      {
        ...environment,
        RUNTROL_TEST_EXTERNAL_INTEGRATION_APPROVAL: integrationApproval,
      },
      180_000,
      approval.signal,
    ),
  ]);
  const result = JSON.parse(await readFile(resultPath, "utf8"));
  if (typeof result.failure === "string") {
    throw new Error(
      `${phase} Extension Host failed after ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  if (result.phase !== phase || result.extensionVersion !== version || result.session !== selectedSession) {
    throw new Error(`${phase} returned inconsistent evidence: ${JSON.stringify(result)}`);
  }
}

function bundledCoreAt(extensionDirectory) {
  return path.join(
    extensionDirectory,
    "resources",
    "core",
    process.platform === "win32" ? "runtrol.exe" : "runtrol",
  );
}

function runCore(arguments_) {
  const result = spawnSync(managedCore, arguments_, {
    env: environment,
    encoding: "utf8",
    timeout: 20_000,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(`Core ${arguments_[0]} failed: ${result.stderr || result.stdout}`);
  }
  return result.stdout.trim();
}

async function exactDaemonPid() {
  const expected = normalizedExecutable(managedCore);
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const matches = processRows().filter((row) =>
      normalizedExecutable(row.executable) === expected && /(?:^|\s)daemon(?:\s|$)/u.test(row.command)
    );
    if (matches.length === 1) {
      return matches[0].pid;
    }
    await delay(100);
  }
  throw new Error("the isolated managed Core daemon could not be identified exactly");
}

async function exactProviderPid(rootPid) {
  const expected = normalizedExecutable(fixture);
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const rows = processRows();
    const descendants = descendantPids(rows, rootPid);
    const matches = rows.filter(
      (row) => descendants.has(row.pid) && normalizedExecutable(row.executable) === expected,
    );
    if (matches.length === 1) {
      return matches[0].pid;
    }
    await delay(100);
  }
  throw new Error("the isolated ACP provider process could not be identified exactly");
}

async function requireSameProcesses(phase) {
  const rows = processRows();
  const pids = new Set(rows.map((row) => row.pid));
  if (!pids.has(daemonPid) || !pids.has(providerPid)) {
    throw new Error(`${phase} stopped the original daemon or provider process`);
  }
  if (!descendantPids(rows, daemonPid).has(providerPid)) {
    throw new Error(`${phase} detached the provider from the original daemon containment tree`);
  }
}

async function exists(file) {
  try {
    await access(file);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") return false;
    throw error;
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
