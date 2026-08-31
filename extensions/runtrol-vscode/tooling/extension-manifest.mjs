import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
export const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const sourceManifest = JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8"));
export const extensionReleasePolicy = Object.freeze(
  JSON.parse(await readFile(path.join(extensionRoot, "release-policy.json"), "utf8")),
);
const releaseVersion = validateReleaseVersion(extensionReleasePolicy.version, extensionReleasePolicy);
export const packageManifest = Object.freeze(
  {
    ...sourceManifest,
    version: extensionReleasePolicy.version,
    contributes: releaseContributions(sourceManifest.contributes, extensionReleasePolicy.version),
  },
);
export const extensionReleaseTag = `${extensionReleasePolicy.tagPrefix}${extensionReleasePolicy.version}`;
export const previousExtensionVersion = [
  releaseVersion.major,
  releaseVersion.minor,
  releaseVersion.patch - extensionReleasePolicy.increment,
].join(".");
export const previousExtensionReleaseTag = `${extensionReleasePolicy.tagPrefix}${previousExtensionVersion}`;

if (!manifestToken(packageManifest.publisher) || !manifestToken(packageManifest.name)) {
  throw new Error("the extension manifest has an invalid Marketplace identity");
}
if (sourceManifest.version !== "0.0.0") {
  throw new Error("the checked-in extension version must stay at the derived-version placeholder 0.0.0");
}

export const extensionIdentifier = `${packageManifest.publisher}.${packageManifest.name}`;
export const extensionInstallPrefix = `${extensionIdentifier}-`;

/// Put the derived release version in the one host-rendered sidebar header.
///
/// A WebviewView description is not rendered when VS Code merges the sole view into its container header, and
/// assigning the view title makes VS Code insert a colon. The packaged container title is the supported surface
/// that produces `Runtrol 0.1.42` exactly. The checked-in manifest remains the version-neutral development SSOT.
function releaseContributions(contributes, version) {
  const containers = contributes?.viewsContainers;
  const activitybar = containers?.activitybar;
  if (!Array.isArray(activitybar)) return contributes;
  const title = `Runtrol ${version}`;
  const runtrolViews = contributes?.views?.runtrol;
  return {
    ...contributes,
    viewsContainers: {
      ...containers,
      activitybar: activitybar.map((container) => (
        container?.id === "runtrol"
          ? { ...container, title }
          : container
      )),
    },
    views: Array.isArray(runtrolViews)
      ? {
          ...contributes.views,
          runtrol: runtrolViews.map((view) => (
            view?.id === "runtrol.sidebar" ? { ...view, name: title } : view
          )),
        }
      : contributes.views,
  };
}

function manifestToken(value) {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]*$/u.test(value);
}

function validateReleaseVersion(version, policy) {
  const expectedKeys = ["increment", "initialPatch", "major", "minor", "tagPrefix", "version"];
  if (JSON.stringify(Object.keys(policy).sort()) !== JSON.stringify(expectedKeys)) {
    throw new Error("release-policy.json has an unknown or missing field");
  }
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.exec(version);
  const major = Number(match?.[1]);
  const minor = Number(match?.[2]);
  const patch = Number(match?.[3]);
  if (
    !Number.isSafeInteger(policy.major)
    || !Number.isSafeInteger(policy.minor)
    || !Number.isSafeInteger(policy.initialPatch)
    || policy.increment !== 1
    || typeof policy.tagPrefix !== "string"
    || !/^[a-z0-9.-]+$/u.test(policy.tagPrefix)
  ) {
    throw new Error("release-policy.json has an invalid patch-only policy");
  }
  if (
    !match
    || major !== policy.major
    || minor !== policy.minor
    || patch < policy.initialPatch + policy.increment
  ) {
    throw new Error(
      `the extension release must stay on ${policy.major}.${policy.minor}.x and advance from its initial patch`,
    );
  }
  return { major, minor, patch };
}
