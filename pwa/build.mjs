import { cp, mkdir, readFile, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const output = join(root, "dist");
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(join(root, "src"), join(output, "src"), { recursive: true });
for (const file of ["index.html", "styles.css", "manifest.webmanifest", "service-worker.js"]) {
  await cp(join(root, file), join(output, file));
}
await mkdir(join(output, "assets", "brand"), { recursive: true });
for (const file of [
  "apple-touch-icon.png",
  "favicon.svg",
  "icon-192.png",
  "icon-512.png",
  "lockup-dark.svg",
  "lockup-light.svg",
]) {
  await cp(join(root, "..", "assets", "brand", file), join(output, "assets", "brand", file));
}
await mkdir(join(output, "assets"), { recursive: true });
await cp(
  join(root, "..", "assets", "event-presentation.json"),
  join(output, "assets", "event-presentation.json"),
);

for (const file of ["app.js", "bytes.js", "core.js", "identityStore.js", "missions.js", "noise.js", "pairing.js", "presentation.js", "push.js", "records.js", "relay.js"]) {
  const source = await readFile(join(output, "src", file), "utf8");
  if (source.includes("\u2013") || source.includes("\u2014")) {
    throw new Error(`${file} contains forbidden punctuation`);
  }
}

for (const file of ["index.html", "styles.css", "manifest.webmanifest", "service-worker.js"]) {
  const source = await readFile(join(output, file), "utf8");
  if (source.includes("\u2013") || source.includes("\u2014")) {
    throw new Error(`${file} contains forbidden punctuation`);
  }
}

console.log(`Built ${output}.`);
