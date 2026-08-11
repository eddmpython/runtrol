import { cp, mkdir, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const target = process.argv[2];
if (target !== "win32-x64") {
  throw new Error(`unsupported package target ${target ?? "<missing>"}`);
}

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const source = process.env.RUNTROL_CORE_BINARY
  ? path.resolve(process.env.RUNTROL_CORE_BINARY)
  : path.join(repositoryRoot, "target/release/runtrol.exe");
await stat(source);

const coreDirectory = path.join(extensionRoot, "resources/core");
await mkdir(coreDirectory, { recursive: true });
await cp(source, path.join(coreDirectory, "runtrol.exe"));

const build = spawnSync(process.execPath, [path.join(extensionRoot, "tooling/build.mjs")], {
  cwd: extensionRoot,
  stdio: "inherit",
});
if (build.status !== 0) {
  process.exit(build.status ?? 1);
}

const release = path.join(repositoryRoot, "release");
await mkdir(release, { recursive: true });
const output = path.join(release, `runtrol-studio-0.0.0-${target}.vsix`);
const vsce = path.join(extensionRoot, "node_modules/@vscode/vsce/vsce");
const packaged = spawnSync(process.execPath, [vsce, "package", "--target", target, "--out", output], {
  cwd: extensionRoot,
  stdio: "inherit",
});
process.exitCode = packaged.status ?? 1;
