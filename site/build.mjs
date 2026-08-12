import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const siteRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = dirname(siteRoot);
const outputRoot = join(siteRoot, "dist");
const brandSource = join(repositoryRoot, "assets", "brand");
const brandOutput = join(outputRoot, "assets", "brand");
const sourceFiles = ["index.html", "styles.css", "app.js", "release-assets.mjs"];
const brandFiles = [
  "apple-touch-icon.png",
  "favicon.ico",
  "favicon.svg",
  "lockup-dark.svg",
  "lockup-light.svg",
  "social-card-dark.png",
  "symbol-orange.svg",
];

await rm(outputRoot, { recursive: true, force: true });
await mkdir(brandOutput, { recursive: true });

for (const name of sourceFiles) {
  await cp(join(siteRoot, name), join(outputRoot, name));
}
for (const name of brandFiles) {
  await cp(join(brandSource, name), join(brandOutput, name));
}
await writeFile(join(outputRoot, ".nojekyll"), "", "utf8");

const html = await readFile(join(outputRoot, "index.html"), "utf8");
for (const requiredPath of ["styles.css", "app.js", ...brandFiles.map((name) => `assets/brand/${name}`)]) {
  if (!html.includes(requiredPath) && !["symbol-orange.svg", "lockup-dark.svg", "lockup-light.svg"].includes(requiredPath.split("/").at(-1))) {
    throw new Error(`built page does not reference required asset: ${requiredPath}`);
  }
}

async function totalBytes(path) {
  let bytes = 0;
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const item = join(path, entry.name);
    if (entry.isDirectory()) {
      bytes += await totalBytes(item);
    } else {
      bytes += (await readFile(item)).byteLength;
    }
  }
  return bytes;
}

const bytes = await totalBytes(outputRoot);
const budget = 250_000;
if (bytes > budget) {
  throw new Error(`site output ${bytes} bytes exceeds ${budget} byte budget`);
}

console.log(`Built ${outputRoot} (${bytes} bytes, no runtime dependencies).`);
