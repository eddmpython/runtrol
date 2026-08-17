import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, readdir, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

import { isolatedExtensionTestArguments } from "./isolated-vscode.mjs";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const out = path.join(extensionRoot, ".test-dist");
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
verifyExtensionTestArguments();

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
process.exitCode = result.status ?? 1;

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
