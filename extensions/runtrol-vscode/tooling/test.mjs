import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, readdir, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

import {
  isConvergentSignalError,
  isolatedExtensionTestArguments,
  ownedTreeIdentities,
  terminateCapturedIdentities,
} from "./isolated-vscode.mjs";
import { descendantPids, WINDOWS_PROCESS_ROWS_MAX_BUFFER_BYTES } from "./process-identity.mjs";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const out = path.join(extensionRoot, ".test-dist");
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
verifyExtensionTestArguments();
verifyConvergentSignalErrors();
verifyProcessIdentityBuffer();
verifyProcessGenerationBoundary();
await verifyOwnedProcessTreeCleanup();

// Every test beside the code it tests, discovered rather than listed.
//
// A hand-written runner list is a second place the truth lives, and the copy that goes stale skips a whole file
// without failing. Discovery cannot skip a file that exists.
//
// One directory is deliberately not here: the suites under `src/integration` import the real `vscode` module and
// can only run inside an Extension Host, which `tooling/extension-host.mjs` owns. Excluding them by name keeps the
// two runners from silently splitting a suite between them.
const HOST_DRIVEN = "integration";
const suites = await discoverSuites(path.join(extensionRoot, "src"));
assert.ok(suites.length > 0, "the extension has unit tests to run");
assert.ok(
  suites.every((suite) => !suite.name.startsWith(`${HOST_DRIVEN}-`)),
  "Extension Host suites belong to the host runner",
);

await build({
  entryPoints: Object.fromEntries(suites.map((suite) => [suite.name, suite.source])),
  outdir: out,
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  alias: {
    "@runtrol/runtime-client": path.join(repositoryRoot, "clients/typescript/src/index.ts"),
  },
  sourcemap: false,
  logLevel: "silent",
});

const result = spawnSync(process.execPath, [
  "--test",
  ...suites.map((suite) => path.join(out, `${suite.name}.js`)),
], {
  stdio: "inherit",
});
await rm(out, { recursive: true, force: true });

// The tooling's own unit suites (plain .mjs, no bundling), run in a second `node --test` pass so a tool
// like inspect-vscode is gated by the same `npm test` as the extension. Discovered, never listed.
const toolingSuites = (await readdir(path.join(extensionRoot, "tooling")))
  .filter((name) => name.endsWith(".test.mjs"))
  .sort()
  .map((name) => path.join(extensionRoot, "tooling", name));
const toolingResult = toolingSuites.length > 0
  ? spawnSync(process.execPath, ["--test", ...toolingSuites], { stdio: "inherit" })
  : { status: 0 };

process.exitCode = (result.status ?? 1) || (toolingResult.status ?? 1);

async function discoverSuites(root) {
  const found = [];
  const entries = await readdir(root, { recursive: true, withFileTypes: true });
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith(".test.ts")) continue;
    const source = path.join(entry.parentPath ?? entry.path, entry.name);
    if (path.relative(root, source).split(path.sep).includes(HOST_DRIVEN)) continue;
    found.push({ name: path.relative(root, source).replaceAll(path.sep, "-").slice(0, -3), source });
  }
  return found.sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0));
}

// Core spawns one provider fixture per session as its own child, so the Extension Host harness owns
// a tree rather than a process. Whether killing the root also reaps that tree belongs to the host,
// not to the harness: this machine's shell puts every spawn in one job object and reaps them
// together, while a plain desktop run leaves the children behind. The harness therefore may not
// depend on either, and the contract tested here is the one it does depend on. A snapshot taken
// while the tree was alive terminates every process in it, descendants included.
//
// The root is deliberately left running until the sweep, so nothing but the sweep can end the tree
// and the assertion cannot pass by accident on a host that reaps children on its own.
async function verifyOwnedProcessTreeCleanup() {
  const marker = "runtrol probe leaf";
  const leaf = `setTimeout(() => {}, 600000); /* ${marker} */`;
  const rootScript = "const { spawn } = require('node:child_process');"
    + `spawn(process.execPath, ['-e', ${JSON.stringify(leaf)}], { stdio: 'ignore' });`
    + "setTimeout(() => {}, 600000);";
  const root = spawn(process.execPath, ["-e", rootScript], { stdio: "ignore", windowsHide: true });
  let captured = [];
  try {
    const deadline = Date.now() + 30_000;
    let descendant;
    while (Date.now() < deadline) {
      captured = ownedTreeIdentities(root.pid);
      // The root's own command line carries the marker too, since it spells the leaf out to spawn it.
      descendant = captured.find(
        (identity) => identity.pid !== root.pid && identity.command.includes(marker),
      );
      if (descendant) break;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    assert.ok(descendant, "the probe tree exposes the descendant spawned below its root");
    assert.ok(processExists(root.pid), "the probe root is still running when the sweep starts");
    assert.ok(processExists(descendant.pid), "the descendant is still running when the sweep starts");

    await terminateCapturedIdentities(captured);
    await waitForProcessExit(captured.map((identity) => identity.pid), 5_000);
    assert.ok(
      captured.every((identity) => !processExists(identity.pid)),
      "the captured snapshot terminates the whole tree, descendants included",
    );
  } finally {
    for (const pid of [root.pid, ...captured.map((identity) => identity.pid)]) {
      try {
        process.kill(pid, "SIGKILL");
      } catch (error) {
        // Teardown of the probe tree. ESRCH is the state this is trying to reach, so only a
        // different failure means the probe left something behind.
        if (error.code !== "ESRCH") throw error;
      }
    }
  }
}

function verifyConvergentSignalErrors() {
  assert.equal(isConvergentSignalError({ code: "ESRCH" }, "linux"), true);
  assert.equal(isConvergentSignalError({ code: "EPERM" }, "win32"), true);
  assert.equal(isConvergentSignalError({ code: "EPERM" }, "linux"), false);
  assert.equal(isConvergentSignalError({ code: "EACCES" }, "win32"), false);
}

function verifyProcessIdentityBuffer() {
  assert.equal(
    WINDOWS_PROCESS_ROWS_MAX_BUFFER_BYTES,
    16 * 1024 * 1024,
    "a busy Windows host has bounded headroom for the complete identity snapshot",
  );
}

function verifyProcessGenerationBoundary() {
  const rows = [
    { pid: 10, ppid: 1, startedAt: 20_000 },
    { pid: 20, ppid: 10, startedAt: 20_100 },
    { pid: 30, ppid: 20, startedAt: 20_200 },
    // Its parent PID happens to equal the new root PID, but this process predates that root generation.
    { pid: 40, ppid: 10, startedAt: 1_000 },
    { pid: 50, ppid: 40, startedAt: 20_300 },
  ];
  assert.deepEqual(
    [...descendantPids(rows, 10)].sort((left, right) => left - right),
    [20, 30],
    "a stale parent PID cannot turn an older process tree into owned cleanup work",
  );
  assert.deepEqual([...descendantPids(rows, 99)], [], "an absent root grants no process ownership");
}

async function waitForProcessExit(pids, milliseconds) {
  const deadline = Date.now() + milliseconds;
  while (pids.some(processExists) && Date.now() < deadline) {
    // On Unix a terminated direct child keeps its PID until Node receives the close event and reaps it. Yielding
    // here distinguishes that already-terminated state from a process that survived the cleanup signal.
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

function verifyExtensionTestArguments() {
  const arguments_ = isolatedExtensionTestArguments({
    workspace: "/workspace",
    userData: "/profile/user",
    extensions: "/profile/extensions",
    testEntry: "/extension/tests.cjs",
    extensionRoot: "/extension",
  });
  assert.equal(arguments_[0], "/workspace");
  assert.equal(arguments_.includes("--new-window"), false);
  assert.equal(arguments_.includes("--user-data-dir"), false);
  assert.equal(arguments_.includes("--extensions-dir"), false);
  assert.equal(arguments_.includes("--extensionTestsPath"), false);
  assert.equal(arguments_.includes("--extensionDevelopmentPath"), false);
  assert.equal(arguments_.includes("--user-data-dir=/profile/user"), true);
  assert.equal(arguments_.includes("--extensions-dir=/profile/extensions"), true);
  assert.equal(arguments_.includes("--extensionTestsPath=/extension/tests.cjs"), true);
  assert.equal(arguments_.includes("--extensionDevelopmentPath=/extension"), true);
}
