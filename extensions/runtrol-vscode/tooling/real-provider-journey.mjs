import { spawnSync } from "node:child_process";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { runTests } from "@vscode/test-electron";
import { build } from "esbuild";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const output = path.join(extensionRoot, ".test-dist");
const testEntry = path.join(output, "realProviderJourney.test.cjs");
const core = requiredEnvironment("RUNTROL_TEST_CORE");
const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
const firstWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_ONE");
const secondWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_TWO");
const userData = requiredEnvironment("RUNTROL_VSCODE_USER_DATA");
const extensions = requiredEnvironment("RUNTROL_VSCODE_EXTENSIONS");

await Promise.all([
  stat(core),
  stat(firstWorkspace),
  stat(secondWorkspace),
  mkdir(path.join(userData, "User"), { recursive: true }),
  mkdir(extensions, { recursive: true }),
]);
await writeFile(
  path.join(userData, "User", "settings.json"),
  JSON.stringify({
    "runtrol.corePath": core,
    "runtrol.followWorkspace": true,
    "workbench.startupEditor": "none",
  }),
  "utf8",
);

const bundled = spawnSync(process.execPath, [path.join(extensionRoot, "tooling", "build.mjs")], {
  cwd: extensionRoot,
  encoding: "utf8",
  windowsHide: true,
});
if (bundled.status !== 0) {
  throw new Error(`production extension build failed:\n${bundled.stdout}${bundled.stderr}`);
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

try {
  const firstError = await runHost(firstWorkspace).then(() => null, (error) => error);
  let result = await readResult();
  if (typeof result.failure === "string") {
    throw new Error(
      `journey failed at ${String(result.stage)}: ${result.failure}`
      + (typeof result.stack === "string" ? `\n${result.stack}` : ""),
    );
  }
  if (result.stage === "switching") {
    await runHost(secondWorkspace);
    result = await readResult();
  } else if (firstError) {
    throw firstError;
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

async function runHost(workspace) {
  await runTests({
    cachePath: path.join(extensionRoot, ".vscode-test"),
    extensionDevelopmentPath: extensionRoot,
    extensionTestsPath: testEntry,
    extensionTestsEnv: process.env,
    launchArgs: [
      workspace,
      "--disable-extensions",
      `--user-data-dir=${userData}`,
      `--extensions-dir=${extensions}`,
    ],
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || "stable",
    vscodeExecutablePath: process.env.RUNTROL_TEST_VSCODE_EXECUTABLE || undefined,
  });
}

async function readResult() {
  const raw = await readFile(resultPath, "utf8");
  const result = JSON.parse(raw);
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    throw new Error("the Extension Host journey result is not an object");
  }
  return result;
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}
