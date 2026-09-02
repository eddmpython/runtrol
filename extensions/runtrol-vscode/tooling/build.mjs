import { cp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const dist = path.join(extensionRoot, "dist");
const resources = path.join(extensionRoot, "resources");
const providerIcons = path.join(resources, "provider-icons");
const actionIcons = path.join(resources, "action-icons");
const codicons = path.join(extensionRoot, "node_modules", "@vscode", "codicons");
const includeTestJourney = process.env.RUNTROL_INCLUDE_TEST_JOURNEY === "1";

await rm(dist, { recursive: true, force: true });
await rm(providerIcons, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
await mkdir(resources, { recursive: true });
await mkdir(providerIcons, { recursive: true });
await Promise.all([
  cp(path.join(repositoryRoot, "assets/brand/symbol.svg"), path.join(resources, "symbol.svg")),
  cp(path.join(repositoryRoot, "assets/brand/icon-512.png"), path.join(resources, "icon.png")),
  cp(path.join(repositoryRoot, "LICENSE"), path.join(resources, "LICENSE")),
  // NOTICE carries the agreement for the CA root data the Core embeds. It has to travel with the
  // binary, and LICENSE cannot carry it: text beyond the license itself stops scanners from
  // identifying the license at all.
  cp(path.join(repositoryRoot, "NOTICE"), path.join(resources, "NOTICE")),
  cp(path.join(codicons, "LICENSE"), path.join(resources, "CODICONS_LICENSE.txt")),
  cp(path.join(codicons, "dist", "codicon.css"), path.join(dist, "codicon.css")),
  cp(path.join(codicons, "dist", "codicon.ttf"), path.join(dist, "codicon.ttf")),
]);

await buildProviderIcons();

await Promise.all([
  build({
    entryPoints: [path.join(extensionRoot, "src/extension.ts")],
    outfile: path.join(dist, "extension.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    // QR generation is a pairing-only feature. Its bounded sibling bundle is loaded by pairingQr.ts on demand,
    // instead of charging every Extension Host activation for the encoder tables.
    external: ["vscode", "./pairingQrVendor"],
    alias: {
      "@runtrol/runtime-client": path.join(repositoryRoot, "clients/typescript/src/index.ts"),
    },
    define: {
      RUNTROL_INCLUDE_TEST_JOURNEY: JSON.stringify(includeTestJourney),
    },
    minify: true,
    sourcemap: false,
    logLevel: "info",
  }),
  build({
    // The `vscode:uninstall` hook: plain Node, no VS Code API, run on the start after Studio was removed.
    entryPoints: [path.join(extensionRoot, "src/uninstall.ts")],
    outfile: path.join(dist, "uninstall.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    minify: true,
    sourcemap: false,
    logLevel: "info",
  }),
  build({
    entryPoints: [path.join(extensionRoot, "src/pairingQrVendor.ts")],
    outfile: path.join(dist, "pairingQrVendor.js"),
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    minify: true,
    sourcemap: false,
    logLevel: "info",
  }),
]);

async function buildProviderIcons() {
  const sprite = await readFile(path.join(codicons, "dist", "codicon.svg"), "utf8");
  const symbols = new Map();
  for (const match of sprite.matchAll(/<symbol\b([^>]*)\bid="([a-z0-9-]+)"([^>]*)>([\s\S]*?)<\/symbol>/gu)) {
    const attributes = `${match[1] ?? ""}${match[3] ?? ""}`
      .replaceAll(/\s+xmlns="[^"]*"/gu, "")
      .trim();
    symbols.set(match[2], { attributes, body: match[4] ?? "" });
  }
  const fallback = symbols.get("sparkle");
  if (!fallback) throw new Error("the pinned Codicons package has no sparkle glyph");

  const names = new Set(["sparkle"]);
  const manifests = path.join(repositoryRoot, "crates", "runtrol-drivers", "manifests");
  for (const entry of await readdir(manifests, { withFileTypes: true })) {
    if (!entry.isFile() || path.extname(entry.name) !== ".toml") continue;
    const manifest = await readFile(path.join(manifests, entry.name), "utf8");
    const icon = /^icon\s*=\s*"([a-z0-9-]{1,64})"\s*$/mu.exec(manifest)?.[1];
    if (icon) names.add(icon);
  }

  // Deleting a conversation is the one row action that does not come back, so its control is drawn in the
  // editor's own error colour rather than in the foreground grey every other action shares. A menu icon cannot
  // be tinted through a theme token the way a tree item can, so the colour is baked into the file here, from
  // the same pinned glyph set the service icons come from.
  await mkdir(actionIcons, { recursive: true });
  const trash = symbols.get("trash");
  if (!trash) throw new Error("the pinned Codicons package has no trash glyph");
  await writeFile(
    path.join(actionIcons, "trash.svg"),
    `<?xml version="1.0" encoding="utf-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" ${trash.attributes}>\n`
      + `<style>:root{color:#f14c4c}</style>\n${trash.body}\n</svg>\n`,
    "utf8",
  );

  await Promise.all([...names].map(async (name) => {
    const target = path.join(providerIcons, `${name}.svg`);
    // A repo-owned brand icon wins for a service the editor's icon font carries no mark for. Present means use
    // it; absent (stat rejects) means this name has no override and the editor glyph is rendered instead.
    const brand = path.join(repositoryRoot, "assets", "brand", "provider-icons", `${name}.svg`);
    const hasBrand = await stat(brand).then(() => true, () => false);
    if (hasBrand) {
      await cp(brand, target);
      return;
    }
    const glyph = symbols.get(name) ?? fallback;
    const svg = `<?xml version="1.0" encoding="utf-8"?>\n`
      + `<svg xmlns="http://www.w3.org/2000/svg" ${glyph.attributes}>\n`
      + "<style>:root{color:#424242}@media(prefers-color-scheme:dark){:root{color:#c5c5c5}}</style>\n"
      + `${glyph.body}\n</svg>\n`;
    await writeFile(target, svg, "utf8");
  }));
}
