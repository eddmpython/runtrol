import { execFile } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const packageRoot = new URL("..", import.meta.url);
const packagePath = fileURLToPath(packageRoot);
const scratch = await mkdtemp(join(tmpdir(), "runtrol-runtime-client-consumer-"));

try {
  const packed = JSON.parse((await npm([
    "pack",
    "--json",
    "--pack-destination",
    scratch,
  ], packageRoot)).stdout);
  const archive = join(scratch, packed[0].filename);
  const consumer = join(scratch, "consumer");
  await mkdir(consumer);
  await writeFile(join(consumer, "package.json"), JSON.stringify({
    name: "runtime-client-packed-consumer",
    private: true,
    type: "module",
  }));
  await npm([
    "install",
    "--offline",
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
    archive,
  ], consumer);
  await writeFile(join(consumer, "verify.ts"), `
    import {
      RuntimeConnector,
      type ListNativeSessionsParams,
      type ProviderId,
      type RuntimeClient,
    } from "@runtrol/runtime-client";
    import { ScriptedRuntimeTransport } from "@runtrol/runtime-client/testing";
    const provider: ProviderId = "opaque-provider";
    void provider;
    new RuntimeConnector();
    new ScriptedRuntimeTransport([]).close();
    declare const runtime: RuntimeClient;
    const nativePage: ListNativeSessionsParams = { providerId: provider, root: "C:/opaque-root" };
    void runtime.providers().listNativeSessions(nativePage);
    // @ts-expect-error raw protocol dispatch is not part of the public client API
    runtime.call("providers/list", {});
  `);
  await execute(process.execPath, [
    join(packagePath, "node_modules", "typescript", "bin", "tsc"),
    "--noEmit",
    "--strict",
    "--skipLibCheck",
    "--target",
    "ES2022",
    "--module",
    "NodeNext",
    "--moduleResolution",
    "NodeNext",
    "verify.ts",
  ], { cwd: consumer, windowsHide: true });
  await writeFile(join(consumer, "verify.mjs"), `
    import { FINALIZED_REVISIONS, PUBLIC_LIMITS } from "@runtrol/runtime-client";
    import { ScriptedRuntimeTransport } from "@runtrol/runtime-client/testing";
    import schema from "@runtrol/runtime-client/schema" with { type: "json" };
    if (!FINALIZED_REVISIONS.length || PUBLIC_LIMITS.maxFrameBytes < 1) process.exit(1);
    if (!schema.$defs?.InitializeResult) process.exit(2);
    new ScriptedRuntimeTransport([]).close();
  `);
  await execute(process.execPath, ["verify.mjs"], { cwd: consumer, windowsHide: true });
} finally {
  await rm(scratch, { recursive: true, force: true });
}

async function npm(arguments_, cwd) {
  const npmCli = process.env.npm_execpath;
  if (!npmCli) throw new Error("npm_execpath is unavailable");
  return execute(process.execPath, [npmCli, ...arguments_], {
    cwd,
    windowsHide: true,
    maxBuffer: 4 * 1024 * 1024,
  });
}
