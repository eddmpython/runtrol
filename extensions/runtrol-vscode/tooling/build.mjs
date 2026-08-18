import { cp, mkdir, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const dist = path.join(extensionRoot, "dist");
const resources = path.join(extensionRoot, "resources");

await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
await mkdir(resources, { recursive: true });
await Promise.all([
  cp(path.join(repositoryRoot, "assets/brand/symbol.svg"), path.join(resources, "symbol.svg")),
  cp(path.join(repositoryRoot, "assets/brand/icon-512.png"), path.join(resources, "icon.png")),
  cp(path.join(repositoryRoot, "LICENSE"), path.join(resources, "LICENSE")),
  // NOTICE carries the agreement for the CA root data the Core embeds. It has to travel with the
  // binary, and LICENSE cannot carry it: text beyond the license itself stops scanners from
  // identifying the license at all.
  cp(path.join(repositoryRoot, "NOTICE"), path.join(resources, "NOTICE")),
]);

await Promise.all([
  build({
    entryPoints: [path.join(extensionRoot, "src/extension.ts")],
    outfile: path.join(dist, "extension.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    external: ["vscode"],
    alias: {
      "@runtrol/runtime-client": path.join(repositoryRoot, "clients/typescript/src/index.ts"),
    },
    minify: true,
    sourcemap: false,
    logLevel: "info",
  }),
  build({
    entryPoints: [path.join(extensionRoot, "src/webview/main.ts")],
    outfile: path.join(dist, "webview.js"),
    bundle: true,
    platform: "browser",
    format: "iife",
    target: "es2022",
    minify: true,
    sourcemap: false,
    logLevel: "info",
  }),
]);
