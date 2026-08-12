import { mkdir, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const out = path.join(extensionRoot, ".test-dist");
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

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
