import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

import {
  isolatedExtensionTestArguments,
  temporarilyDisableVSCodeGallery,
} from "./isolated-vscode.mjs";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const out = path.join(extensionRoot, ".test-dist");
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
await verifyMacProductIsolation();
verifyExtensionTestArguments();

await build({
  entryPoints: {
    "framing.test": path.join(extensionRoot, "src/core/framing.test.ts"),
    "liveCore.test": path.join(extensionRoot, "src/core/liveCore.test.ts"),
    "managedCore.test": path.join(extensionRoot, "src/core/managedCore.test.ts"),
    "presentation.test": path.join(extensionRoot, "src/webview/presentation.test.ts"),
    "renderReady.test": path.join(extensionRoot, "src/webview/renderReady.test.ts"),
    "selectionStore.test": path.join(extensionRoot, "src/selectionStore.test.ts"),
    "sessionNavigation.test": path.join(extensionRoot, "src/sessionNavigation.test.ts"),
    "stateRows.test": path.join(extensionRoot, "src/stateRows.test.ts"),
    "workspaceCollision.test": path.join(extensionRoot, "src/workspaceCollision.test.ts"),
  },
  outdir: out,
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  sourcemap: false,
  logLevel: "silent",
});

const result = spawnSync(process.execPath, [
  "--test",
  path.join(out, "framing.test.js"),
  path.join(out, "liveCore.test.js"),
  path.join(out, "managedCore.test.js"),
  path.join(out, "presentation.test.js"),
  path.join(out, "renderReady.test.js"),
  path.join(out, "selectionStore.test.js"),
  path.join(out, "sessionNavigation.test.js"),
  path.join(out, "stateRows.test.js"),
  path.join(out, "workspaceCollision.test.js"),
], {
  stdio: "inherit",
});
await rm(out, { recursive: true, force: true });
process.exitCode = result.status ?? 1;

async function verifyMacProductIsolation() {
  const source = path.join(out, "Source Code.app");
  const executable = path.join(source, "Contents", "MacOS", "Electron");
  const product = path.join(source, "Contents", "Resources", "app", "product.json");
  const backup = `${product}.runtrol-gallery-backup`;
  await Promise.all([
    mkdir(path.dirname(executable), { recursive: true }),
    mkdir(path.dirname(product), { recursive: true }),
  ]);
  const original = JSON.stringify({ name: "Code", extensionsGallery: { serviceUrl: "https://invalid" } });
  await Promise.all([
    writeFile(executable, "binary", "utf8"),
    writeFile(product, original, "utf8"),
  ]);

  const restore = await temporarilyDisableVSCodeGallery(executable);
  const isolatedProduct = JSON.parse(await readFile(product, "utf8"));
  assert.equal(isolatedProduct.name, "Code");
  assert.equal("extensionsGallery" in isolatedProduct, false);
  await restore();
  assert.equal(await readFile(product, "utf8"), original);
  await restore();
  assert.equal(await readFile(product, "utf8"), original);

  const interrupted = JSON.stringify({ name: "Interrupted", extensionsGallery: { serviceUrl: "saved" } });
  await Promise.all([
    writeFile(product, JSON.stringify({ name: "Damaged" }), "utf8"),
    writeFile(backup, interrupted, "utf8"),
  ]);
  const restoreAfterInterruption = await temporarilyDisableVSCodeGallery(executable);
  assert.equal(JSON.parse(await readFile(product, "utf8")).name, "Interrupted");
  await restoreAfterInterruption();
  assert.equal(await readFile(product, "utf8"), interrupted);
  await assert.rejects(readFile(backup), { code: "ENOENT" });
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
