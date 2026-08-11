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
  entryPoints: [
    path.join(extensionRoot, "src/core/framing.test.ts"),
    path.join(extensionRoot, "src/core/liveCore.test.ts"),
  ],
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
], {
  stdio: "inherit",
});
await rm(out, { recursive: true, force: true });
process.exitCode = result.status ?? 1;
