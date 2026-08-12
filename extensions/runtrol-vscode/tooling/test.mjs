import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

import { isolateVSCodeProduct } from "./isolated-vscode.mjs";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const out = path.join(extensionRoot, ".test-dist");
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
await verifyMacProductIsolation();

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
  const helper = path.join(source, "Contents", "Frameworks", "Code Helper.app", "helper");
  await Promise.all([
    mkdir(path.dirname(executable), { recursive: true }),
    mkdir(path.dirname(product), { recursive: true }),
    mkdir(path.dirname(helper), { recursive: true }),
  ]);
  await Promise.all([
    writeFile(executable, "binary", "utf8"),
    writeFile(product, JSON.stringify({ name: "Code", extensionsGallery: { serviceUrl: "https://invalid" } }), "utf8"),
    writeFile(helper, "helper", "utf8"),
  ]);

  const destination = path.join(out, "Isolated Code.app");
  const isolatedExecutable = await isolateVSCodeProduct(executable, destination);
  assert.equal(isolatedExecutable, path.join(destination, "Contents", "MacOS", "Electron"));
  await stat(path.join(destination, "Contents", "Frameworks", "Code Helper.app", "helper"));
  const isolatedProduct = JSON.parse(await readFile(
    path.join(destination, "Contents", "Resources", "app", "product.json"),
    "utf8",
  ));
  const sourceProduct = JSON.parse(await readFile(product, "utf8"));
  assert.equal(isolatedProduct.name, "Code");
  assert.equal("extensionsGallery" in isolatedProduct, false);
  assert.equal("extensionsGallery" in sourceProduct, true);
}
