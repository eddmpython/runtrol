import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

import { runtimeLocatorAt } from "../src/testing.js";

const executeFile = promisify(execFile);

test("an owner-only locator produces one validated local endpoint", async () => {
  const scratch = await mkdtemp(join(tmpdir(), "runtrol-ts-locator-"));
  const path = join(scratch, "runtime.locator.json");
  const endpoint = process.platform === "win32"
    ? "\\\\.\\pipe\\runtrol-runtime-fixture"
    : join(scratch, "runtrol-runtime.sock");
  try {
    await writeFile(path, JSON.stringify({
      schema: 1,
      instanceId: `rtm_${"4".repeat(32)}`,
      endpointKind: process.platform === "win32" ? "namedPipe" : "unixSocket",
      endpoint,
      runtimeVersion: "0.1.1",
      processId: process.pid,
    }));
    await makeOwnerOnly(path);
    const state = await runtimeLocatorAt(path).inspect();
    assert.equal(state.state, "running");
    if (state.state === "running") assert.equal(state.locator.endpoint, endpoint);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

async function makeOwnerOnly(path: string): Promise<void> {
  if (process.platform !== "win32") {
    await chmod(path, 0o600);
    return;
  }
  const systemRoot = process.env.SystemRoot;
  assert.ok(systemRoot && isAbsolute(systemRoot));
  const powershell = join(
    systemRoot,
    "System32",
    "WindowsPowerShell",
    "v1.0",
    "powershell.exe",
  );
  const script = [
    "& { param([string]$TargetPath)",
    "$ErrorActionPreference='Stop'",
    "$acl=[System.IO.File]::GetAccessControl($TargetPath)",
    "$acl.SetAccessRuleProtection($true,$false)",
    "$identity=[Security.Principal.WindowsIdentity]::GetCurrent().User",
    "$rule=New-Object Security.AccessControl.FileSystemAccessRule($identity,'FullControl','Allow')",
    "$acl.SetAccessRule($rule)",
    "[System.IO.File]::SetAccessControl($TargetPath,$acl)",
    "}",
  ].join(";");
  await executeFile(
    powershell,
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script, path],
    { windowsHide: true },
  );
}
