import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runTests } from "@vscode/test-electron";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const core = process.env.RUNTROL_TEST_CORE
  ? path.resolve(process.env.RUNTROL_TEST_CORE)
  : path.join(repositoryRoot, "target", "debug", process.platform === "win32" ? "runtrol.exe" : "runtrol");
await stat(core);

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`production extension build failed:\n${bundled.stdout}${bundled.stderr}`);
}

// macOS expands its per-user temporary directory to a path long enough to exceed the Unix-domain socket
// ceiling once runtrol's home and socket names are appended. `/tmp` is the kernel-stable short alias for
// exactly this purpose, and the random suffix still isolates concurrent runs.
const temporaryRoot = process.platform === "darwin" ? "/tmp" : os.tmpdir();
const temporary = await mkdtemp(path.join(temporaryRoot, "runtrol-vscode-host-"));
const output = path.join(extensionRoot, ".test-dist");
const testEntry = path.join(output, "extensionHost.test.cjs");
const resultPath = path.join(temporary, "result.json");
const runtrolHome = path.join(temporary, "runtrol-home");
const userData = path.join(temporary, "user");
const extensions = path.join(temporary, "extensions");
await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await mkdir(path.join(userData, "User"), { recursive: true });
await mkdir(extensions, { recursive: true });
await writeFile(
  path.join(userData, "User", "settings.json"),
  JSON.stringify({ "runtrol.corePath": core, "workbench.startupEditor": "none" }),
  "utf8",
);
let daemon = null;
let daemonStderr = "";

try {
  daemon = spawn(core, ["daemon"], {
    env: { ...process.env, RUNTROL_HOME: runtrolHome },
    stdio: ["ignore", "ignore", "pipe"],
    windowsHide: true,
  });
  daemon.stderr.setEncoding("utf8").on("data", (chunk) => {
    daemonStderr += chunk;
  });
  await delay(500);
  if (daemon.exitCode !== null) {
    throw new Error(`test Core stopped during startup:\n${daemonStderr}`);
  }
  const reached = spawnSync(core, ["endpoint"], {
    env: { ...process.env, RUNTROL_HOME: runtrolHome },
    encoding: "utf8",
    timeout: 15_000,
    windowsHide: true,
  });
  if (reached.status !== 0 || !reached.stdout.trim()) {
    throw new Error(`test Core did not expose an endpoint:\n${reached.stdout}${reached.stderr}`);
  }
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "extensionHost.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });

  const testEnvironment = {
    RUNTROL_HOME: runtrolHome,
    RUNTROL_TEST_CORE: core,
    RUNTROL_VSCODE_RESULT: resultPath,
  };
  const installed = process.env.RUNTROL_TEST_VSCODE_EXECUTABLE;
  if (installed?.toLowerCase().endsWith(".cmd")) {
    await runInstalledCode(installed, testEntry, resultPath, testEnvironment);
  } else {
    await runTests({
      cachePath: path.join(extensionRoot, ".vscode-test"),
      extensionDevelopmentPath: extensionRoot,
      extensionTestsPath: testEntry,
      extensionTestsEnv: testEnvironment,
      launchArgs: [
        repositoryRoot,
        "--disable-extensions",
        `--user-data-dir=${userData}`,
        `--extensions-dir=${extensions}`,
      ],
      version: process.env.RUNTROL_TEST_VSCODE_VERSION || "stable",
      vscodeExecutablePath: installed || undefined,
    });
  }

  const result = JSON.parse(await readFile(resultPath, "utf8"));
  process.stdout.write(`RUNTROL_VSCODE_HOST ${JSON.stringify(result)}\n`);
} catch (error) {
  if (daemon?.exitCode !== null) {
    throw new Error(`the VS Code host run failed after Core exited with ${String(daemon?.exitCode)}`, {
      cause: error,
    });
  }
  const crash = await readFile(path.join(runtrolHome, "daemon-crash.log"), "utf8").catch(
    (readError) => readError.code === "ENOENT" ? "" : Promise.reject(readError),
  );
  if (crash) {
    throw new Error(`the VS Code host run failed after a Core crash:\n${crash}`, { cause: error });
  }
  if (daemonStderr) {
    throw new Error(`the VS Code host run failed and Core reported:\n${daemonStderr}`, { cause: error });
  }
  throw error;
} finally {
  if (daemon?.exitCode === null) {
    const exited = new Promise((resolve) => daemon.once("close", resolve));
    daemon.kill();
    await Promise.race([
      exited,
      delay(5_000).then(() => Promise.reject(new Error("test Core did not terminate within 5 seconds"))),
    ]);
  }
  await rm(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function runInstalledCode(executable, testEntry, resultPath, testEnvironment) {
  const arguments_ = [
    "--new-window",
    "--disable-extensions",
    "--disable-workspace-trust",
    "--skip-welcome",
    "--user-data-dir",
    userData,
    "--extensions-dir",
    extensions,
    "--extensionDevelopmentPath",
    extensionRoot,
    "--extensionTestsPath",
    testEntry,
    repositoryRoot,
  ];
  const started = new Promise((resolve, reject) => {
    const child = spawn(`"${executable}"`, arguments_, {
      env: { ...process.env, ...testEnvironment },
      shell: true,
      stdio: "inherit",
      windowsHide: true,
    });
    child.once("error", reject);
    child.once("spawn", resolve);
  });
  await started;

  const deadline = Date.now() + 30_000;
  let lastStage = "not started";
  while (Date.now() < deadline) {
    try {
      const result = JSON.parse(await readFile(resultPath, "utf8"));
      if (typeof result.vscode === "string") {
        await delay(1_000);
        return;
      }
      if (typeof result.failure === "string") {
        throw new Error(
          `installed VS Code test failed after checkpoint ${String(result.stage || lastStage)}: ${result.failure}`
          + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
        );
      }
      if (typeof result.stage === "string") {
        lastStage = result.stage;
      }
    } catch (error) {
      if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) {
        throw error;
      }
    }
    await delay(100);
  }
  throw new Error(`installed VS Code test timed out after checkpoint ${lastStage}`);
}
