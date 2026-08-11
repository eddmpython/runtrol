import { cp, mkdir, readFile, rm, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const packageManifest = JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8"));
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
  ? path.resolve(process.env.RUNTROL_CORE_BINARY)
  : path.join(repositoryRoot, "target", "release", targetContract.executable);
const sourceInfo = await stat(source);
if (!sourceInfo.isFile() || sourceInfo.size < 1024 * 1024) {
  throw new Error(`the release Core at ${source} is missing or too small to be the product binary`);
}

const coreDirectory = path.join(extensionRoot, "resources/core");
await rm(coreDirectory, { recursive: true, force: true });
await mkdir(coreDirectory, { recursive: true });
await cp(source, path.join(coreDirectory, targetContract.executable));

const build = spawnSync(process.execPath, [path.join(extensionRoot, "tooling/build.mjs")], {
  cwd: extensionRoot,
  stdio: "inherit",
});
if (build.status !== 0) {
  process.exit(build.status ?? 1);
}

const release = path.join(repositoryRoot, "release");
await mkdir(release, { recursive: true });
const output = path.join(release, `${packageManifest.name}-${packageManifest.version}-${target}.vsix`);
const vsce = path.join(extensionRoot, "node_modules/@vscode/vsce/vsce");
const packaged = spawnSync(
  process.execPath,
  [vsce, "package", "--target", target, "--no-dependencies", "--out", output],
  { cwd: extensionRoot, stdio: "inherit" },
);
process.exitCode = packaged.status ?? 1;
