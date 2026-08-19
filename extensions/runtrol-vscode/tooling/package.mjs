import { cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

import { extensionRoot, packageManifest, repositoryRoot } from "./extension-manifest.mjs";

const targets = JSON.parse(await readFile(path.join(extensionRoot, "release-targets.json"), "utf8"));
const nativeTarget = `${process.platform}-${process.arch}`;
const target = process.argv[2] ?? nativeTarget;
const targetContract = targets[target];
if (!targetContract) {
  throw new Error(`unsupported package target ${target}`);
}
if (!process.env.RUNTROL_CORE_BINARY && target !== nativeTarget) {
  throw new Error(
    `target ${target} does not match this ${nativeTarget} host. Set RUNTROL_CORE_BINARY to a verified cross-built Core.`,
  );
}

const source = process.env.RUNTROL_CORE_BINARY
  ? path.resolve(repositoryRoot, process.env.RUNTROL_CORE_BINARY)
  : path.join(repositoryRoot, "target", "vscode-release", "release", targetContract.executable);
const sourceInfo = await stat(source);
if (!sourceInfo.isFile() || sourceInfo.size < 1024 * 1024) {
  throw new Error(`the release Core at ${source} is missing or too small to be the product binary`);
}

const build = spawnSync(process.execPath, [path.join(extensionRoot, "tooling/build.mjs")], {
  cwd: extensionRoot,
  stdio: "inherit",
});
if (build.status !== 0) {
  process.exit(build.status ?? 1);
}

// The repository-root release directory is the release CI's artifact contract. Local rehearsal
// gates override it into the ignored build tree so a rehearsal never leaves a stray artifact for
// the hygiene gate to flag.
const release = process.env.RUNTROL_PACKAGE_OUTPUT_DIR ?? path.join(repositoryRoot, "release");
await mkdir(release, { recursive: true });
const output = path.join(release, `${packageManifest.name}-${packageManifest.version}-${target}.vsix`);
const vsce = path.join(extensionRoot, "node_modules/@vscode/vsce/vsce");
const staging = await mkdtemp(path.join(os.tmpdir(), "runtrol-vsix-"));
try {
  const stagedDist = path.join(staging, "dist");
  const stagedResources = path.join(staging, "resources");
  const stagedCore = path.join(stagedResources, "core");
  await Promise.all([
    mkdir(stagedDist, { recursive: true }),
    mkdir(stagedCore, { recursive: true }),
  ]);
  await Promise.all([
    writeFile(path.join(staging, "package.json"), `${JSON.stringify(packageManifest, null, 2)}\n`, "utf8"),
    cp(path.join(extensionRoot, "README.md"), path.join(staging, "README.md")),
    cp(path.join(extensionRoot, ".vscodeignore"), path.join(staging, ".vscodeignore")),
    cp(path.join(extensionRoot, "dist"), stagedDist, { recursive: true }),
    cp(path.join(extensionRoot, "resources/icon.png"), path.join(stagedResources, "icon.png")),
    cp(path.join(extensionRoot, "resources/symbol.svg"), path.join(stagedResources, "symbol.svg")),
    cp(path.join(extensionRoot, "resources/LICENSE"), path.join(stagedResources, "LICENSE")),
    cp(path.join(extensionRoot, "resources/NOTICE"), path.join(stagedResources, "NOTICE")),
    cp(source, path.join(stagedCore, targetContract.executable)),
  ]);
  const packaged = spawnSync(
    process.execPath,
    [vsce, "package", "--target", target, "--no-dependencies", "--out", output],
    { cwd: staging, stdio: "inherit" },
  );
  process.exitCode = packaged.status ?? 1;
} finally {
  await rm(staging, { recursive: true, force: true });
}
