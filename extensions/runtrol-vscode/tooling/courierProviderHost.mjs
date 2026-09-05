// An isolated native Extension Host for real-provider courier journeys.
// Usage: node tooling/courierProviderHost.mjs --core <development executable> [--project <existing project> | --resume <retained host>]
// The printed coordination folder speaks ownerReveal.test.ts. A stop.json file requests complete cleanup.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { cp, mkdir, mkdtemp, open, readFile, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { extensionIdentifier, extensionRoot, packageManifest } from "./extension-manifest.mjs";
import { acquireVSCode, isolatedExtensionTestArguments, isolatedProfileSettings, isolatedRuntimeState,
  ownedTreeIdentities, terminateCapturedIdentities } from "./isolated-vscode.mjs";
import { normalizedExecutable, processRows } from "./process-identity.mjs";
import { HOST_DEADLINE_ENV, HOST_LIFETIME_MS, waitForHostStop } from "./courierHostLifetime.mjs";
import { claimHostRun, readRetainedHost, retainedHostRoot } from "./courierHostResume.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const coreIndex = process.argv.indexOf("--core");
assert.ok(coreIndex >= 0 && process.argv[coreIndex + 1], "--core is required");
const original = path.resolve(process.argv[coreIndex + 1]);
await stat(original);
const projectIndex = process.argv.indexOf("--project");
const resumeIndex = process.argv.indexOf("--resume");
assert.ok(projectIndex < 0 || resumeIndex < 0, "--resume reuses its recorded project; do not combine it with --project");
let existingProject = null;
if (projectIndex >= 0) {
  const requested = process.argv[projectIndex + 1];
  assert.ok(requested && path.isAbsolute(requested), "--project requires an absolute existing directory");
  existingProject = await realpath(requested);
  assert.ok((await stat(existingProject)).isDirectory(), "--project must name a directory");
}
const executionRoot = path.join(process.env.LOCALAPPDATA, "dev-workspace");
const temporary = resumeIndex >= 0
  ? await retainedHostRoot(process.argv[resumeIndex + 1], executionRoot)
  : await mkdtemp(path.join(executionRoot, "runtrolProvider-"));
let workspace;
let coordination;
const development = path.join(temporary, "extension");
const userData = path.join(temporary, "profile");
const core = path.join(temporary, "bin", "runtrol.exe");
const probe = path.join(temporary, "bin", "handoverProbe.exe");
const identity = path.join(temporary, "identity.json");
const { home, environment } = isolatedRuntimeState(temporary);
const processes = [];
const logs = [];
// Failure and expiry preserve the state needed to recover native conversations and owned worktrees.
let leaveEvidence = true;
const releaseClaim = await claimHostRun(temporary);
try {
  const previous = resumeIndex >= 0 ? await readRetainedHost(temporary, processRows()) : null;
  workspace = previous?.workspace ?? existingProject ?? path.join(temporary, "project");
  coordination = previous
    ? await mkdtemp(path.join(temporary, "coordination-"))
    : path.join(temporary, "coordination");
  if (previous) {
    // Keep the original descriptor beside its original receipts before replacing the current-run pointer.
    await cp(path.join(temporary, "host.json"), path.join(previous.coordination, "host.json"), { force: false });
  }
  const folders = [coordination, development, path.dirname(core), path.join(userData, "User")];
  if (existingProject === null && previous === null) folders.push(workspace);
  for (const folder of folders) {
    await mkdir(folder, { recursive: true });
  }
  await cp(original, core);
  await cp(path.join(path.dirname(original), "examples", "handoverProbe.exe"), probe);
  await cp(path.join(extensionRoot, "dist"), path.join(development, "dist"), { recursive: true });
  await cp(path.join(extensionRoot, "resources"), path.join(development, "resources"), { recursive: true });
  await writeFile(path.join(development, "package.json"), JSON.stringify(packageManifest));
  await build({ entryPoints: [path.join(extensionRoot, "src/extension.ts")],
    outfile: path.join(development, "dist/extension.js"), bundle: true, platform: "node", format: "cjs",
    target: "node20", external: ["vscode", "./pairingQrVendor"], minify: true,
    alias: { "@runtrol/runtime-client": path.join(root, "clients/typescript/src/index.ts") },
    define: { RUNTROL_INCLUDE_TEST_JOURNEY: "true" }, logLevel: "silent" });
  const testEntry = path.join(coordination, "journey.cjs");
  await build({ entryPoints: [path.join(extensionRoot, "src/integration/ownerReveal.test.ts")], outfile: testEntry,
    bundle: true, platform: "node", format: "cjs", target: "node20", external: ["vscode"], logLevel: "silent" });
  const settings = previous
    ? JSON.parse(await readFile(path.join(userData, "User", "settings.json"), "utf8"))
    : isolatedProfileSettings;
  await writeFile(path.join(userData, "User", "settings.json"), JSON.stringify({ ...settings,
    "runtrol.corePath": core, "window.title": "Runtrol courier verification" }));
  const { executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test"));
  // Preparation owns no live Runtime. The one immutable lifetime starts immediately before the first spawn.
  const deadlineAtMs = Date.now() + HOST_LIFETIME_MS;
  await start(core, ["daemon"], "runtime", environment, true);
  await start(executable, isolatedExtensionTestArguments({ workspace, userData,
    extensions: path.join(temporary, "extensions"), testEntry, extensionRoot: development, visual: true }), "viewer", {
    ...environment, RUNTROL_TEST_CORE: core, RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    RUNTROL_VSCODE_COORDINATION: coordination, RUNTROL_VSCODE_ROLE: "viewer",
    [HOST_DEADLINE_ENV]: String(deadlineAtMs),
    RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1", RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]),
  }, false);
  const descriptor = { temporary, workspace, coordination, home, core, probe, identity, deadlineAtMs,
    logs: { runtime: path.join(coordination, "runtime.log"), viewer: path.join(coordination, "viewer.log") },
    resumedFrom: previous?.coordination ?? null, processes: processes.map(({ identity, label }) => ({ ...identity, label })) };
  await writeFile(path.join(coordination, "host.json"), JSON.stringify(descriptor));
  const pointer = path.join(temporary, `host-${process.pid}.json`);
  await writeFile(pointer, JSON.stringify(descriptor));
  await rename(pointer, path.join(temporary, "host.json"));
  process.stdout.write(`RUNTROL_PROVIDER_HOST ${JSON.stringify(descriptor)}\n`);
  const requestedEvidence = await waitForHostStop(coordination, processes, deadlineAtMs);
  leaveEvidence = previous !== null || requestedEvidence;
} catch (error) {
  if (coordination) await writeFile(path.join(coordination, "host-failure.json"), JSON.stringify({
    atMs: Date.now(), failure: error instanceof Error ? error.message : String(error),
  }));
  process.stderr.write(`RUNTROL_PROVIDER_HOST_FAILED ${error instanceof Error ? error.message : String(error)}\n`);
  throw error;
} finally {
  const cleanupErrors = [];
  for (const entry of processes.reverse()) {
    try {
      const tree = ownedTreeIdentities(entry.child.pid);
      const current = tree.find((row) => row.pid === entry.child.pid);
      if (current && entry.identity && current.startedAt === entry.identity.startedAt
        && normalizedExecutable(current.executable) === normalizedExecutable(entry.binary)) {
        await terminateCapturedIdentities(tree);
      } else if (current) throw new Error(`cannot prove cleanup ownership for ${entry.child.pid}`);
    } catch (error) {
      // A failed viewer cleanup must not skip the separately owned Runtime and its provider processes.
      cleanupErrors.push(error);
    }
  }
  for (const log of logs) {
    try { await log.close(); }
    catch (error) { cleanupErrors.push(error); }
  }
  if (cleanupErrors.length === 0 && !leaveEvidence) {
    // Revoke the recoverable descriptor under the claim before deleting a successfully disposable host.
    try { await rm(path.join(temporary, "host.json")); }
    catch (error) { cleanupErrors.push(error); }
  }
  try { await releaseClaim(); }
  catch (error) { cleanupErrors.push(error); }
  if (cleanupErrors.length > 0) {
    throw new AggregateError(cleanupErrors, `owned provider host cleanup failed; evidence retained at ${temporary}`);
  }
  if (!leaveEvidence) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  process.stdout.write(`RUNTROL_PROVIDER_HOST_CLOSED ${JSON.stringify({ temporary, retained: leaveEvidence })}\n`);
}

async function start(binary, words, label, environment, windowsHide) {
  const log = await open(path.join(coordination, `${label}.log`), "w"); logs.push(log);
  const child = spawn(binary, words, { cwd: root, env: environment, windowsHide, stdio: ["ignore", log.fd, log.fd] });
  const entry = { child, binary, label, identity: null }; processes.push(entry);
  entry.identity = processRows().find((row) => row.pid === child.pid && normalizedExecutable(row.executable) === normalizedExecutable(binary));
  if (!entry.identity) throw new Error(`cannot prove birth identity of ${label} ${child.pid}`);
  child.on("error", (error) => process.stderr.write(`${label}: ${error.message}\n`));
  child.on("exit", (code, signal) => process.stdout.write(
    `RUNTROL_PROVIDER_PROCESS_EXIT ${JSON.stringify({ label, pid: child.pid, code, signal })}\n`,
  ));
}
