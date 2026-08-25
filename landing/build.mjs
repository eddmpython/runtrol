// Builds landing/dist: the same output shape as site/build.mjs (minus the phone app, which the Pages
// job copies from pwa/dist). Promotion into site/ is a file move, not a rewrite, because the relative
// asset paths are identical.

import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const landingRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = dirname(landingRoot);
const outputRoot = join(landingRoot, "dist");
const brandSource = join(repositoryRoot, "assets", "brand");
const brandOutput = join(outputRoot, "assets", "brand");
const sourceFiles = ["index.html", "styles.css", "app.js", "icons.js", "scene.js"];
const brandFiles = ["apple-touch-icon.png", "favicon.ico", "favicon.svg", "social-card-dark.png"];

await rm(outputRoot, { recursive: true, force: true });
await mkdir(brandOutput, { recursive: true });

for (const name of sourceFiles) {
  await cp(join(landingRoot, name), join(outputRoot, name));
}
await cp(join(repositoryRoot, "site", "release-assets.mjs"), join(outputRoot, "release-assets.mjs"));
for (const name of brandFiles) {
  await cp(join(brandSource, name), join(brandOutput, name));
}
await writeFile(join(outputRoot, ".nojekyll"), "", "utf8");

const html = await readFile(join(outputRoot, "index.html"), "utf8");
for (const requiredPath of ["styles.css", "app.js", "app/", ...brandFiles.map((name) => `assets/brand/${name}`)]) {
  if (!html.includes(requiredPath)) {
    throw new Error(`built page does not reference required asset: ${requiredPath}`);
  }
}

async function totalBytes(path) {
  let bytes = 0;
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const item = join(path, entry.name);
    bytes += entry.isDirectory() ? await totalBytes(item) : (await readFile(item)).byteLength;
  }
  return bytes;
}

const bytes = await totalBytes(outputRoot);
const budget = 200_000;
if (bytes > budget) {
  throw new Error(`landing output ${bytes} bytes exceeds ${budget} byte budget`);
}

console.log(`Built ${outputRoot} (${bytes} bytes, no runtime dependencies).`);
