import { spawn, spawnSync } from "node:child_process";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";

import { build } from "esbuild";

import { extensionIdentifier, extensionRoot } from "./extension-manifest.mjs";
import {
  isolatedExtensionTestArguments,
  isolatedProfileSettings,
  terminateExactProcesses,
  withoutHostIdentity,
} from "./isolated-vscode.mjs";

const output = path.join(extensionRoot, ".test-dist");
const testEntry = path.join(output, "realProviderJourney.test.cjs");
const core = requiredEnvironment("RUNTROL_TEST_CORE");
const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
const firstWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_ONE");
const secondWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_TWO");
const userData = requiredEnvironment("RUNTROL_VSCODE_USER_DATA");
const extensions = requiredEnvironment("RUNTROL_VSCODE_EXTENSIONS");
const configuredVscode = requiredEnvironment("RUNTROL_TEST_VSCODE_EXECUTABLE");
const repositoryRoot = path.resolve(extensionRoot, "../..");
const publicSchema = JSON.parse(await readFile(
  path.join(repositoryRoot, "crates", "runtrol-runtime-protocol", "schema", "runtime.schema.json"),
  "utf8",
));
const controlLeaseLifetimeMs = publicSchema?.["x-runtrol-limits"]?.controlLeaseLifetimeMs;
if (!Number.isSafeInteger(controlLeaseLifetimeMs) || controlLeaseLifetimeMs <= 0) {
  throw new Error("the public schema has no valid control lease lifetime");
}
// Stripped rather than copied whole. A VS Code launched with the outer host's identity starts as plain Node and
// never loads a workbench, which is how this journey came to fail with a missing module named after its workspace.
const testEnvironment = withoutHostIdentity();

await Promise.all([
  stat(core),
  stat(configuredVscode),
  stat(firstWorkspace),
  stat(secondWorkspace),
  mkdir(path.join(userData, "User"), { recursive: true }),
  mkdir(extensions, { recursive: true }),
]);
const vscode = configuredVscode;
try {
  await writeFile(
    path.join(userData, "User", "settings.json"),
    JSON.stringify({
      ...isolatedProfileSettings,
      "runtrol.corePath": core,
      "runtrol.followWorkspace": true,
    }),
    "utf8",
  );

  const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
    cwd: extensionRoot,
    env: { ...process.env, RUNTROL_INCLUDE_TEST_JOURNEY: "1" },
    encoding: "utf8",
    windowsHide: true,
  });
  if (bundled.status !== 0) {
    throw new Error(`test extension build failed:\n${bundled.stdout}${bundled.stderr}`);
  }

  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  await build({
    entryPoints: [path.join(extensionRoot, "src", "integration", "realProviderJourney.test.ts")],
    outfile: testEntry,
    bundle: true,
    external: ["vscode"],
    platform: "node",
    format: "cjs",
    target: "node20",
    sourcemap: false,
    logLevel: "silent",
  });

  await runHost(firstWorkspace, "switching");
  let result = await readResult();
  if (typeof result.failure === "string") {
    throw new Error(
      `journey failed at ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  if (result.stage === "switching") {
    // Isolated Extension Hosts may not share native SecretStorage even with one profile. If the next host receives a
    // fresh integration, the old principal keeps control until the protocol lease expires because disconnect never
    // transfers authority. Wait that exact schema-owned lifetime before the next host enrolls.
    await delay(controlLeaseLifetimeMs + 1_000);
    await runHost(secondWorkspace, "complete");
    result = await readResult();
  }
  if (typeof result.failure === "string") {
    throw new Error(
      `journey failed at ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  if (result.stage !== "complete") {
    throw new Error(`journey stopped at ${String(result.stage)}`);
  }
  process.stdout.write(`RUNTROL_VSCODE_REAL_PROVIDER ${JSON.stringify(result)}\n`);
} finally {
  await rm(output, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

async function runHost(workspace, expectedStage) {
  const arguments_ = isolatedExtensionTestArguments({
    workspace,
    userData,
    extensions,
    testEntry,
    extensionRoot,
  });
  const child = spawn(vscode, arguments_, {
    env: {
      ...testEnvironment,
      RUNTROL_TEST_EXTENSION_ID: extensionIdentifier,
    },
    stdio: "inherit",
    windowsHide: true,
  });
  let exit = null;
  let spawnError = null;
  child.once("error", (error) => {
    spawnError = error;
  });
  child.once("exit", (code, signal) => {
    exit = { code, signal, at: Date.now() };
  });
  try {
    const deadline = Date.now() + 150_000;
    let lastStage = "not started";
    while (Date.now() < deadline) {
      if (spawnError) {
        throw spawnError;
      }
      const result = await tryReadResult();
      if (result) {
        if (typeof result.stage === "string") {
          lastStage = result.stage;
        }
        if (typeof result.failure === "string") {
          throw new Error(
            `journey failed at ${String(result.stage)}: ${result.failure}`
            + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
          );
        }
        if (result.stage === expectedStage) {
          if (expectedStage === "switching") {
            await delay(5_000);
          }
          return;
        }
      }
      if (exit && Date.now() - exit.at > 2_000) {
        throw new Error(
          `VS Code exited as ${String(exit.code ?? exit.signal)} after checkpoint ${lastStage}`,
        );
      }
      await delay(100);
    }
    throw new Error(`VS Code timed out after checkpoint ${lastStage}`);
  } finally {
    await terminateExactProcesses(userData, null);
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
    }
  }
}

async function readResult() {
  const raw = await readFile(resultPath, "utf8");
  const result = JSON.parse(raw);
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    throw new Error("the Extension Host journey result is not an object");
  }
  return result;
}

async function tryReadResult() {
  try {
    return await readResult();
  } catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) {
      return null;
    }
    throw error;
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}
