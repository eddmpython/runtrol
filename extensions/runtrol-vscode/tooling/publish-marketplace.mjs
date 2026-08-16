import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

import {
  extensionIdentifier,
  extensionRoot,
  packageManifest,
  repositoryRoot,
} from "./extension-manifest.mjs";

const VSIX_DIGEST_PROPERTY = "Microsoft.VisualStudio.Services.VsixSha256";
const VERIFY_DEADLINE_MS = 180_000;
const VERIFY_INTERVAL_MS = 5_000;
const RELEASE_DIRECTORY = path.join(repositoryRoot, "release");
const TRUSTED_REPOSITORY = repositorySlug(packageManifest.repository);
const TRUSTED_WORKFLOW_REF = `${TRUSTED_REPOSITORY}/.github/workflows/vscode-release.yml@refs/heads/main`;
const vsce = path.join(extensionRoot, "node_modules", "@vscode", "vsce", "vsce");
const targets = JSON.parse(
  await readFile(path.join(extensionRoot, "release-targets.json"), "utf8"),
);

if (process.argv.includes("--selftest")) {
  selftest();
} else {
  await publish(directoryArgument(process.argv.slice(2)));
}

async function publish(directory) {
  requireGitHubOIDC(process.env);
  const archives = await exactArchives(directory);
  const expectedDigests = new Map(
    await Promise.all(
      archives.map(async ({ target, archive }) => [target, await fileDigest(archive)]),
    ),
  );
  const published = spawnSync(
    process.execPath,
    [
      vsce,
      "publish",
      "--oidc",
      "--skip-duplicate",
      "--packagePath",
      ...archives.map(({ archive }) => archive),
    ],
    {
      cwd: extensionRoot,
      env: process.env,
      encoding: "utf8",
      timeout: 15 * 60_000,
      windowsHide: true,
    },
  );
  process.stdout.write(published.stdout ?? "");
  process.stderr.write(published.stderr ?? "");
  if (published.status !== 0) {
    throw new Error(`Marketplace OIDC publishing failed with exit ${String(published.status)}`);
  }

  const deadline = Date.now() + VERIFY_DEADLINE_MS;
  let problems = ["Marketplace verification has not started"];
  while (Date.now() < deadline) {
    try {
      problems = marketplaceProblems(showMarketplace(), expectedDigests);
      if (problems.length === 0) {
        process.stdout.write(
          `RUNTROL_MARKETPLACE_PUBLISHED ${JSON.stringify({
            extension: extensionIdentifier,
            version: packageManifest.version,
            targets: [...expectedDigests.keys()].sort(),
          })}\n`,
        );
        return;
      }
    } catch (error) {
      problems = [error instanceof Error ? error.message : String(error)];
    }
    await delay(VERIFY_INTERVAL_MS);
  }
  throw new Error(`Marketplace did not expose the exact release: ${problems.join("; ")}`);
}

function directoryArgument(arguments_) {
  if (arguments_.length !== 2 || arguments_[0] !== "--directory") {
    throw new Error("usage: publish-marketplace.mjs --directory <release-directory>");
  }
  const directory = path.resolve(repositoryRoot, arguments_[1]);
  if (directory !== RELEASE_DIRECTORY) {
    throw new Error("Marketplace publishing accepts only the repository release directory");
  }
  return directory;
}

function requireGitHubOIDC(environment) {
  for (const name of [
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "GITHUB_ACTIONS",
    "GITHUB_REF",
    "GITHUB_REPOSITORY",
    "GITHUB_WORKFLOW_REF",
  ]) {
    if (typeof environment[name] !== "string" || environment[name].length === 0) {
      throw new Error(`Marketplace publishing requires GitHub Actions OIDC environment ${name}`);
    }
  }
  if (environment.GITHUB_ACTIONS !== "true") {
    throw new Error("Marketplace OIDC publishing runs only inside GitHub Actions");
  }
  if (environment.GITHUB_REF !== "refs/heads/main") {
    throw new Error("Marketplace OIDC publishing runs only from main");
  }
  if (environment.GITHUB_REPOSITORY !== TRUSTED_REPOSITORY) {
    throw new Error("Marketplace OIDC publishing runs only from the manifest repository");
  }
  if (environment.GITHUB_WORKFLOW_REF !== TRUSTED_WORKFLOW_REF) {
    throw new Error("Marketplace OIDC publishing runs only from the trusted workflow on main");
  }
}

function repositorySlug(repository) {
  if (typeof repository !== "object" || repository === null || typeof repository.url !== "string") {
    throw new Error("the extension manifest has no structured repository URL");
  }
  const url = new URL(repository.url);
  const pathParts = url.pathname.replace(/\.git$/u, "").split("/").filter(Boolean);
  if (url.protocol !== "https:" || url.hostname !== "github.com" || pathParts.length !== 2) {
    throw new Error("Marketplace trusted publishing requires an exact GitHub repository URL");
  }
  return pathParts.join("/");
}

async function exactArchives(directory) {
  const actual = (await readdir(directory, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith(".vsix"))
    .map((entry) => entry.name)
    .sort();
  return archiveContracts(directory, actual);
}

function archiveContracts(directory, actual) {
  const expected = Object.keys(targets)
    .sort()
    .map((target) => ({
      target,
      name: `${packageManifest.name}-${packageManifest.version}-${target}.vsix`,
    }));
  const names = expected.map(({ name }) => name);
  if (JSON.stringify(actual) !== JSON.stringify(names)) {
    throw new Error(`the release directory does not contain the exact platform set: ${JSON.stringify(actual)}`);
  }
  return expected.map(({ target, name }) => ({ target, archive: path.join(directory, name) }));
}

function showMarketplace() {
  const shown = spawnSync(
    process.execPath,
    [vsce, "show", extensionIdentifier, "--json"],
    {
      cwd: extensionRoot,
      env: process.env,
      encoding: "utf8",
      timeout: 30_000,
      windowsHide: true,
    },
  );
  if (shown.status !== 0) {
    throw new Error(
      `Marketplace metadata lookup failed with exit ${String(shown.status)}: ${shown.stderr.trim()}`,
    );
  }
  return JSON.parse(shown.stdout);
}

function marketplaceProblems(metadata, expectedDigests) {
  const problems = [];
  if (metadata.publisher?.publisherName !== packageManifest.publisher
    || metadata.extensionName !== packageManifest.name) {
    problems.push("Marketplace answered for a different extension identity");
  }
  const versions = Array.isArray(metadata.versions)
    ? metadata.versions.filter((candidate) => candidate.version === packageManifest.version)
    : [];
  const actualTargets = versions
    .map((candidate) => candidate.targetPlatform)
    .filter((target) => typeof target === "string")
    .sort();
  const expectedTargets = [...expectedDigests.keys()].sort();
  if (JSON.stringify(actualTargets) !== JSON.stringify(expectedTargets)) {
    problems.push(`Marketplace target set is ${JSON.stringify(actualTargets)}`);
  }
  for (const [target, digest] of expectedDigests) {
    const candidates = versions.filter((candidate) => candidate.targetPlatform === target);
    if (candidates.length !== 1) {
      problems.push(`${target} has ${candidates.length} Marketplace entries`);
      continue;
    }
    const properties = Array.isArray(candidates[0].properties) ? candidates[0].properties : [];
    const found = properties.find((property) => property.key === VSIX_DIGEST_PROPERTY)?.value;
    if (found !== digest) {
      problems.push(`${target} has digest ${String(found)}`);
    }
  }
  return problems;
}

async function fileDigest(file) {
  const digest = createHash("sha256");
  digest.update(await readFile(file));
  return digest.digest("hex");
}

function selftest() {
  const expectFailure = (operation) => {
    try {
      operation();
      return 0;
    } catch {
      return 1;
    }
  };
  const githubEnvironment = {
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: "request-token",
    ACTIONS_ID_TOKEN_REQUEST_URL: "https://example.invalid/token",
    GITHUB_ACTIONS: "true",
    GITHUB_REF: "refs/heads/main",
    GITHUB_REPOSITORY: TRUSTED_REPOSITORY,
    GITHUB_WORKFLOW_REF: TRUSTED_WORKFLOW_REF,
  };
  requireGitHubOIDC(githubEnvironment);
  let boundaryMutations = 0;
  for (const name of Object.keys(githubEnvironment)) {
    boundaryMutations += expectFailure(
      () => requireGitHubOIDC({ ...githubEnvironment, [name]: "" }),
    );
  }
  boundaryMutations += expectFailure(
    () => requireGitHubOIDC({ ...githubEnvironment, GITHUB_ACTIONS: "false" }),
  );
  boundaryMutations += expectFailure(
    () => requireGitHubOIDC({ ...githubEnvironment, GITHUB_REF: "refs/heads/different" }),
  );
  boundaryMutations += expectFailure(
    () => requireGitHubOIDC({ ...githubEnvironment, GITHUB_REPOSITORY: "different/repository" }),
  );
  boundaryMutations += expectFailure(
    () => requireGitHubOIDC({ ...githubEnvironment, GITHUB_WORKFLOW_REF: "different" }),
  );
  if (directoryArgument(["--directory", "release"]) !== RELEASE_DIRECTORY) {
    throw new Error("the release directory fixture was rejected");
  }
  boundaryMutations += expectFailure(
    () => directoryArgument(["--directory", "different"]),
  );
  const archiveNames = Object.keys(targets)
    .sort()
    .map((target) => `${packageManifest.name}-${packageManifest.version}-${target}.vsix`);
  if (archiveContracts(RELEASE_DIRECTORY, archiveNames).length !== archiveNames.length) {
    throw new Error("the exact archive set fixture was rejected");
  }
  boundaryMutations += expectFailure(
    () => archiveContracts(RELEASE_DIRECTORY, archiveNames.slice(1)),
  );
  boundaryMutations += expectFailure(
    () => archiveContracts(RELEASE_DIRECTORY, [...archiveNames, "unexpected.vsix"]),
  );
  const expectedBoundaryMutations = Object.keys(githubEnvironment).length + 7;
  if (boundaryMutations !== expectedBoundaryMutations) {
    throw new Error("a publication-boundary mutation escaped");
  }
  const expected = new Map(
    Object.keys(targets).map((target, index) => [target, String(index).padStart(64, "0")]),
  );
  const green = {
    publisher: { publisherName: packageManifest.publisher },
    extensionName: packageManifest.name,
    versions: [...expected].map(([targetPlatform, digest]) => ({
      version: packageManifest.version,
      targetPlatform,
      properties: [{ key: VSIX_DIGEST_PROPERTY, value: digest }],
    })),
  };
  if (marketplaceProblems(green, expected).length !== 0) {
    throw new Error("the green Marketplace fixture was rejected");
  }
  const mutations = [
    { ...green, extensionName: "different" },
    { ...green, versions: green.versions.slice(1) },
    {
      ...green,
      versions: green.versions.map((version, index) => index === 0
        ? { ...version, version: "9.9.9" }
        : version),
    },
    {
      ...green,
      versions: green.versions.map((version, index) => index === 0
        ? { ...version, properties: [{ key: VSIX_DIGEST_PROPERTY, value: "bad" }] }
        : version),
    },
    { ...green, versions: [...green.versions, green.versions[0]] },
  ];
  for (const [index, mutation] of mutations.entries()) {
    if (marketplaceProblems(mutation, expected).length === 0) {
      throw new Error(`Marketplace verification mutation ${index + 1} escaped`);
    }
  }
  process.stdout.write(
    `[publish-marketplace --selftest] OK. ${mutations.length} Marketplace and ${boundaryMutations} publication-boundary mutations fail.\n`,
  );
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
