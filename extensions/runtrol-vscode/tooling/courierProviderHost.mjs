// An isolated native Extension Host for real-provider courier journeys.
// Usage: node tooling/courierProviderHost.mjs --core <development executable>
// The printed coordination folder speaks ownerReveal.test.ts. A stop.json file requests complete cleanup.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { cp, mkdir, mkdtemp, open, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { extensionIdentifier, extensionRoot, packageManifest } from "./extension-manifest.mjs";
import { acquireVSCode, isolatedExtensionTestArguments, isolatedProfileSettings, isolatedRuntimeState,
  ownedTreeIdentities, terminateCapturedIdentities } from "./isolated-vscode.mjs";
import { normalizedExecutable, processRows } from "./process-identity.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const coreIndex = process.argv.indexOf("--core");
assert.ok(coreIndex >= 0 && process.argv[coreIndex + 1], "--core is required");
const original = path.resolve(process.argv[coreIndex + 1]);
await stat(original);
const temporary = await mkdtemp(path.join(process.env.LOCALAPPDATA, "dev-workspace", "runtrolProvider-"));
const workspace = path.join(temporary, "project");
const coordination = path.join(temporary, "coordination");
const development = path.join(temporary, "extension");
const userData = path.join(temporary, "profile");
const core = path.join(temporary, "bin", "runtrol.exe");
const probe = path.join(temporary, "bin", "handoverProbe.exe");
const identity = path.join(temporary, "identity.json");
const { home, environment } = isolatedRuntimeState(temporary);
const processes = [];
const logs = [];
let leaveEvidence = false;
try {
  for (const folder of [workspace, coordination, development, path.dirname(core), path.join(userData, "User")]) {
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
  const testEntry = path.join(temporary, "journey.cjs");
  await build({ entryPoints: [path.join(extensionRoot, "src/integration/ownerReveal.test.ts")], outfile: testEntry,
    bundle: true, platform: "node", format: "cjs", target: "node20", external: ["vscode"], logLevel: "silent" });
  await writeFile(path.join(userData, "User", "settings.json"), JSON.stringify({ ...isolatedProfileSettings,
    "runtrol.corePath": core, "window.title": "Runtrol courier verification" }));
  await start(core, ["daemon"], "runtime", environment, true);
  const { executable } = await acquireVSCode(path.join(extensionRoot, ".vscode-test"));
  await start(executable, isolatedExtensionTestArguments({ workspace, userData,
    extensions: path.join(temporary, "extensions"), testEntry, extensionRoot: development, visual: true }), "viewer", {
    ...environment, RUNTROL_TEST_CORE: core, RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    RUNTROL_VSCODE_COORDINATION: coordination, RUNTROL_VSCODE_ROLE: "viewer",
    RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY: "1", RUNTROL_TEST_INTEGRATION_ROOTS: JSON.stringify([workspace]),
  }, false);
  const descriptor = { temporary, workspace, coordination, home, core, probe, identity, processes: processes.map(({ identity, label }) => ({ ...identity, label })) };
  await writeFile(path.join(temporary, "host.json"), JSON.stringify(descriptor));
  process.stdout.write(`RUNTROL_PROVIDER_HOST ${JSON.stringify(descriptor)}\n`);
  const deadline = Date.now() + 60 * 60 * 1000;
  while (Date.now() < deadline) {
    const stop = await optionalJson(path.join(coordination, "stop.json"));
    if (stop) { leaveEvidence = stop.keepEvidence === true; break; }
    const failed = await optionalJson(path.join(coordination, "viewer-failure.json"));
    if (failed) throw new Error(failed.failure);
    if (processes.some(({ child }) => child.exitCode !== null)) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
} finally {
  for (const entry of processes.reverse()) {
    const tree = ownedTreeIdentities(entry.child.pid);
    const current = tree.find((row) => row.pid === entry.child.pid);
    if (current && entry.identity && current.startedAt === entry.identity.startedAt
      && normalizedExecutable(current.executable) === normalizedExecutable(entry.binary)) {
      await terminateCapturedIdentities(tree);
    } else if (current) throw new Error(`cannot prove cleanup ownership for ${entry.child.pid}`);
  }
  for (const log of logs) await log.close();
  if (!leaveEvidence) await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  process.stdout.write(`RUNTROL_PROVIDER_HOST_CLOSED ${JSON.stringify({ temporary, retained: leaveEvidence })}\n`);
}

async function start(binary, words, label, environment, windowsHide) {
  const log = await open(path.join(temporary, `${label}.log`), "w"); logs.push(log);
  const child = spawn(binary, words, { cwd: root, env: environment, windowsHide, stdio: ["ignore", log.fd, log.fd] });
  const entry = { child, binary, label, identity: null }; processes.push(entry);
  entry.identity = processRows().find((row) => row.pid === child.pid && normalizedExecutable(row.executable) === normalizedExecutable(binary));
  if (!entry.identity) throw new Error(`cannot prove birth identity of ${label} ${child.pid}`);
  child.on("error", (error) => process.stderr.write(`${label}: ${error.message}\n`));
}

async function optionalJson(file) {
  try { return JSON.parse(await readFile(file, "utf8")); }
  catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) return null;
    throw error;
  }
}
