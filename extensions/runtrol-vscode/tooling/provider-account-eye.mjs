// Real task completion with synthetic commands, independent of the Extension Host performance journey.
// Usage: node tooling/provider-account-eye.mjs [--keep-shots] [--cache <VS Code archive cache>]
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { build } from "esbuild";
import { extensionRoot } from "./extension-manifest.mjs";
import {
  acquireVSCode, isolatedExtensionTestArguments, isolatedHostEnvironment, isolatedProfileSettings,
  ownedTreeIdentities, terminateCapturedIdentities,
} from "./isolated-vscode.mjs";

if (process.platform !== "win32") throw new Error("This eye journey uses the Windows capture surface");
const { values } = parseArgs({ options: { "keep-shots": { type: "boolean" }, cache: { type: "string" } } });
if (!process.env.LOCALAPPDATA) throw new Error("LOCALAPPDATA is required");
const executionRoot = path.join(process.env.LOCALAPPDATA, "dev-workspace");
await mkdir(executionRoot, { recursive: true });
const root = await mkdtemp(path.join(executionRoot, "runtrol-account-"));
const result = path.join(root, "result.json");
const userData = path.join(root, "user");
const fixtureExtension = path.join(root, "extension");
let child;
const owned = new Map();
try {
  const { executable } = await acquireVSCode(values.cache ?? path.join(executionRoot, "runtrol-vscode-cache"));
  await Promise.all([path.join(userData, "User"), fixtureExtension, path.join(root, "extensions"),
    path.join(root, "Account Fixture")].map((directory) => mkdir(directory, { recursive: true })));
  await writeFile(path.join(userData, "User", "settings.json"), JSON.stringify({ ...isolatedProfileSettings,
    "security.workspace.trust.enabled": false, "terminal.integrated.defaultProfile.windows": "PowerShell",
    "terminal.integrated.enablePersistentSessions": false }));
  const manifest = JSON.parse(await readFile(path.join(extensionRoot, "package.json"), "utf8"));
  await writeFile(path.join(fixtureExtension, "package.json"), JSON.stringify({ name: "account-action-proof",
    publisher: "runtrol", version: "0.0.0", engines: manifest.engines, main: "extension.js",
    contributes: { taskDefinitions: manifest.contributes.taskDefinitions } }));
  await writeFile(path.join(fixtureExtension, "extension.js"), "exports.activate = () => {};");
  const testEntry = path.join(root, "test.cjs");
  await build({ entryPoints: [path.join(extensionRoot, "src/integration/providerAccountAction.test.ts")],
    outfile: testEntry, bundle: true, platform: "node", format: "cjs", target: "node20", external: ["vscode"] });
  const args = isolatedExtensionTestArguments({ workspace: path.join(root, "Account Fixture"), visual: true,
    userData, extensions: path.join(root, "extensions"), testEntry, extensionRoot: fixtureExtension });
  child = spawn(executable, args, { env: { ...isolatedHostEnvironment(root), RUNTROL_ACCOUNT_ACTION_RESULT: result },
    windowsHide: false, stdio: "ignore" });
  let launchError;
  child.once("error", (error) => { launchError = error; });
  await writeFile(path.join(root, "owner.json"), JSON.stringify({ pid: child.pid, executable, profile: userData, args }));
  console.log(`Owned Code PID ${child.pid}; profile ${userData}`);
  const deadline = Date.now() + 60_000;
  async function readWhenReady(file) {
    for (;;) {
      if (launchError) throw launchError;
      for (const identity of ownedTreeIdentities(child.pid)) owned.set(`${identity.pid}:${identity.started}`, identity);
      try { return await readFile(file, "utf8"); } catch (error) {
        if (error.code !== "ENOENT") throw error;
        if (Date.now() >= deadline || child.exitCode !== null) throw new Error(`Account journey did not produce ${path.basename(file)}`);
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
  }
  await readWhenReady(`${result}.ready`);
  await new Promise((resolve) => setTimeout(resolve, 1800));
  const captured = spawnSync("powershell.exe", ["-NoProfile", "-File", path.join(extensionRoot, "tooling/capture-window.ps1"),
    "-ProcessId", String(child.pid), "-OutPath", path.join(root, "task.png")],
  { encoding: "utf8", windowsHide: true, timeout: 20_000 });
  if (captured.status !== 0) throw new Error(`Account task capture failed: ${captured.error?.message ?? captured.stderr}`);
  await writeFile(`${result}.captured`, "1");
  console.log(`RUNTROL_ACCOUNT_ACTION ${await readWhenReady(result)}`);
} finally {
  if (child?.pid) {
    for (const identity of ownedTreeIdentities(child.pid)) owned.set(`${identity.pid}:${identity.started}`, identity);
    await terminateCapturedIdentities([...owned.values()]);
  }
  const retained = values["keep-shots"] ? new Set(["task.png", "result.json", "owner.json"]) : new Set();
  for (const entry of await readdir(root)) {
    if (!retained.has(entry)) await rm(path.join(root, entry), { recursive: true });
  }
  if (retained.size) console.log(`Retained account evidence: ${root}`);
  else await rm(root, { recursive: true });
}
