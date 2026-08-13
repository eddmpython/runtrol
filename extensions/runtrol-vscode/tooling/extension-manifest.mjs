import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
export const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const sourceManifest = JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8"));
const workspaceManifest = await readFile(path.join(repositoryRoot, "Cargo.toml"), "utf8");
const workspaceVersion = readWorkspaceVersion(workspaceManifest);
export const packageManifest = Object.freeze(
  { ...sourceManifest, version: workspaceVersion },
);

if (!manifestToken(packageManifest.publisher) || !manifestToken(packageManifest.name)) {
  throw new Error("the extension manifest has an invalid Marketplace identity");
}
if (sourceManifest.version !== "0.0.0") {
  throw new Error("the checked-in extension version must stay at the derived-version placeholder 0.0.0");
}

export const extensionIdentifier = `${packageManifest.publisher}.${packageManifest.name}`;
export const extensionInstallPrefix = `${extensionIdentifier}-`;

function manifestToken(value) {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]*$/u.test(value);
}

function readWorkspaceVersion(manifest) {
  const afterHeader = manifest.split(/^\[workspace\.package\]\s*$/mu, 2)[1];
  const section = afterHeader?.split(/^\[/mu, 1)[0];
  const version = section?.match(/^version\s*=\s*"(\d+\.\d+\.\d+)"\s*$/mu)?.[1];
  if (!version || version === "0.0.0") {
    throw new Error("Cargo.toml has no publishable workspace package version");
  }
  return version;
}
