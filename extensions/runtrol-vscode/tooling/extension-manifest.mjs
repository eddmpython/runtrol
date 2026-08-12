import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
export const packageManifest = Object.freeze(
  JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8")),
);

if (!manifestToken(packageManifest.publisher) || !manifestToken(packageManifest.name)) {
  throw new Error("the extension manifest has an invalid Marketplace identity");
}
if (typeof packageManifest.version !== "string" || !packageManifest.version) {
  throw new Error("the extension manifest has no version");
}

export const extensionIdentifier = `${packageManifest.publisher}.${packageManifest.name}`;
export const extensionInstallPrefix = `${extensionIdentifier}-`;

function manifestToken(value) {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]*$/u.test(value);
}
