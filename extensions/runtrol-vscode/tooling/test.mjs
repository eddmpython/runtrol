import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, rm } from "node:fs/promises";
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

await build({
  entryPoints: {
    "framing.test": path.join(extensionRoot, "src/core/framing.test.ts"),
    "client.test": path.join(extensionRoot, "src/core/client.test.ts"),
    "liveCore.test": path.join(extensionRoot, "src/core/liveCore.test.ts"),
    "managedCore.test": path.join(extensionRoot, "src/core/managedCore.test.ts"),
    "presentation.test": path.join(extensionRoot, "src/webview/presentation.test.ts"),
    "sessionDisplay.test": path.join(extensionRoot, "src/sessionDisplay.test.ts"),
    "renderReady.test": path.join(extensionRoot, "src/webview/renderReady.test.ts"),
    "selectionStore.test": path.join(extensionRoot, "src/selectionStore.test.ts"),
    "sessionNavigation.test": path.join(extensionRoot, "src/sessionNavigation.test.ts"),
    "stateRows.test": path.join(extensionRoot, "src/stateRows.test.ts"),
    "workspaceCollision.test": path.join(extensionRoot, "src/workspaceCollision.test.ts"),
    "runtimeProjection.test": path.join(extensionRoot, "src/runtimeProjection.test.ts"),
    "runtimeControl.test": path.join(extensionRoot, "src/runtimeControl.test.ts"),
  },
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
  path.join(out, "framing.test.js"),
  path.join(out, "client.test.js"),
  path.join(out, "liveCore.test.js"),
  path.join(out, "managedCore.test.js"),
  path.join(out, "presentation.test.js"),
  path.join(out, "sessionDisplay.test.js"),
  path.join(out, "renderReady.test.js"),
  path.join(out, "selectionStore.test.js"),
  path.join(out, "sessionNavigation.test.js"),
  path.join(out, "stateRows.test.js"),
  path.join(out, "workspaceCollision.test.js"),
  path.join(out, "runtimeProjection.test.js"),
  path.join(out, "runtimeControl.test.js"),
], {
  stdio: "inherit",
});
await rm(out, { recursive: true, force: true });
process.exitCode = result.status ?? 1;

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
