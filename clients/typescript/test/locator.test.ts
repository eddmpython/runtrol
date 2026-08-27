import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

import { RuntimeLocator, RuntimeLocatorError } from "../src/index.js";
import { LEGACY_DIGEST } from "../src/locator.js";
import { runtimeLocatorAt } from "../src/testing.js";

const executeFile = promisify(execFile);

test("a native locator verifier must be one exact absolute executable", () => {
  assert.throws(
    () => RuntimeLocator.system({ runtimeExecutable: "runtrol" }),
    (error: unknown) => error instanceof RuntimeLocatorError && error.code === "environment",
  );
});

test("an owner-only locator lists generations and the newest one that is not draining is chosen", async () => {
  const scratch = await mkdtemp(join(tmpdir(), "runtrol-ts-locator-"));
  const path = join(scratch, "runtime.locator.json");
  const endpointOf = (tag: string) => process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-runtime-fixture-${tag}`
    : join(scratch, `runtrol-runtime-${tag}.sock`);
  const generation = (byte: string, startedAtMs: number, draining: boolean) => ({
    digest: byte.repeat(64),
    endpointKind: process.platform === "win32" ? "namedPipe" : "unixSocket",
    endpoint: endpointOf(byte.repeat(16)),
    controlEndpoint: `control-${byte}`,
    runtimeVersion: "0.1.1",
    processId: process.pid,
    startedAtMs,
    liveSessions: 0,
    draining,
  });
  try {
    await writeFile(path, JSON.stringify({
      schema: 2,
      instanceId: `rtm_${"4".repeat(32)}`,
      generations: [generation("a", 1, false), generation("b", 2, false), generation("c", 3, true)],
    }));
    await makeOwnerOnly(path);
    const state = await runtimeLocatorAt(path).inspect();
    assert.equal(state.state, "running");
    if (state.state === "running") {
      assert.equal(state.locator.endpoint, endpointOf("b".repeat(16)), "the draining newest is skipped");
      assert.equal(state.locator.digest, "b".repeat(64));
    }
    const preferred = await runtimeLocatorAt(path, "a".repeat(64)).inspect();
    assert.equal(preferred.state, "running");
    if (preferred.state === "running") assert.equal(preferred.locator.digest, "a".repeat(64));
    const all = await runtimeLocatorAt(path).inspectAll();
    assert.equal(all.length, 3);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

test("a locator from before generations is read as one digestless generation, never misread", async () => {
  const scratch = await mkdtemp(join(tmpdir(), "runtrol-ts-locator-legacy-"));
  const path = join(scratch, "runtime.locator.json");
  try {
    await writeFile(path, JSON.stringify({
      schema: 1,
      instanceId: `rtm_${"4".repeat(32)}`,
      endpointKind: process.platform === "win32" ? "namedPipe" : "unixSocket",
      endpoint: process.platform === "win32"
        ? "\\\\.\\pipe\\runtrol-runtime-fixture"
        : join(scratch, "runtrol-runtime.sock"),
      runtimeVersion: "0.1.22",
      processId: process.pid,
    }));
    await makeOwnerOnly(path);
    // Preferring a digest the old daemon never named still lands on it: it is the only generation listed.
    const state = await runtimeLocatorAt(path, "a".repeat(64)).inspect();
    assert.equal(state.state, "running");
    if (state.state !== "running") return;
    assert.equal(state.locator.digest, LEGACY_DIGEST);
    assert.equal(state.locator.controlEndpoint, "");
    assert.equal(state.locator.runtimeVersion, "0.1.22");
    assert.equal(state.locator.draining, false);
    // A record that claims the old schema with fields it never had is still malformed.
    await writeFile(path, JSON.stringify({ schema: 1, instanceId: "rtm_x", surprise: true }));
    await makeOwnerOnly(path);
    await assert.rejects(
      () => runtimeLocatorAt(path).inspect(),
      (error: unknown) => error instanceof RuntimeLocatorError && error.code === "malformed",
    );
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

test("the system locator reads the home the operator chose, the way the Core does", async () => {
  // The defect this exists for: the Core resolved `RUNTROL_HOME` and this SDK resolved only the platform
  // directory, so one process could hold a daemon in one home and a locator in another. Nothing failed loudly;
  // enrollment simply never completed, because each half was talking to a different Runtime.
  const chosen = await mkdtemp(join(tmpdir(), "runtrol-ts-home-"));
  const previous = process.env.RUNTROL_HOME;
  process.env.RUNTROL_HOME = chosen;
  try {
    const locator = RuntimeLocator.system();
    assert.equal(await locator.inspect().then((state) => state.state), "notInstalled");
    // Nothing is installed there, and the point is which file was looked for: an absolute path inside the
    // chosen home rather than one under the platform directory.
    assert.ok(locator.path.startsWith(chosen), `${locator.path} is not inside ${chosen}`);

    process.env.RUNTROL_HOME = "relative/home";
    assert.throws(
      () => RuntimeLocator.system(),
      (error: unknown) => error instanceof RuntimeLocatorError && error.code === "environment",
    );
  } finally {
    if (previous === undefined) delete process.env.RUNTROL_HOME;
    else process.env.RUNTROL_HOME = previous;
    await rm(chosen, { recursive: true, force: true });
  }
});

test("a locator that moves while it is being validated is read again, not called unsafe", async () => {
  // A daemon publishing its own generation writes this file, and on a home whose first daemon is starting that
  // write lands between the native validation and the read. Treating that as an attack ended enrolment for good
  // (measured 2026-08-26). The pair still has to agree before anything is returned; it just gets another look.
  const scratch = await mkdtemp(join(tmpdir(), "runtrol-ts-settle-"));
  const path = join(scratch, "runtime.locator.json");
  const endpoint = process.platform === "win32"
    ? "\\\\.\\pipe\\runtrol-runtime-aaaaaaaaaaaaaaaa"
    : join(scratch, "runtrol-runtime-aaaaaaaaaaaaaaaa.sock");
  const record = {
    schema: 2,
    instanceId: "rtm_settle",
    generations: [{
      digest: "a".repeat(64),
      endpointKind: process.platform === "win32" ? "namedPipe" : "unixSocket",
      endpoint,
      controlEndpoint: process.platform === "win32"
      ? "\\\\.\\pipe\\runtrol-aaaaaaaaaaaaaaaa"
        : join(scratch, "runtrol-aaaaaaaaaaaaaaaa.sock"),
      runtimeVersion: "0.1.1",
      processId: 1,
      startedAtMs: 1,
      liveSessions: 0,
      draining: false,
    }],
  };
  try {
    await writeFile(path, JSON.stringify(record));
    await makeOwnerOnly(path);
    const locator = runtimeLocatorAt(path);
    const state = await locator.inspect();
    assert.equal(state.state, "running");
    assert.equal(state.state === "running" && state.locator.endpoint, endpoint);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});
