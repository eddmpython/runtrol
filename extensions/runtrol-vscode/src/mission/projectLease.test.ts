import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { workspaceIdentity } from "../workspaceCollision";
import { MissionProjectLeases } from "./projectLease";

test("one project has one integration writer while different projects remain independent", async () => {
  const leases = new MissionProjectLeases();
  let release!: () => void;
  const held = leases.run("C:/projects/one", "mission-one", () => new Promise<void>((resolve) => {
    release = resolve;
  }));

  await assert.rejects(
    leases.run("C:/projects/one", "mission-two", async () => undefined),
    /already running for Mission mission-one/,
  );
  await leases.run("C:/projects/two", "mission-two", async () => undefined);

  release();
  await held;
  await leases.run("C:/projects/one", "mission-two", async () => undefined);
});

test("a failed operation always releases its project", async () => {
  const leases = new MissionProjectLeases();
  await assert.rejects(
    leases.run("C:/projects/one", "mission-one", async () => {
      throw new Error("failure");
    }),
    /failure/,
  );
  await leases.run("C:/projects/one", "mission-two", async () => undefined);
});

test("two controller instances share one process-wide project lease", async () => {
  const first = new MissionProjectLeases();
  const second = new MissionProjectLeases();
  let release!: () => void;
  const held = first.run("C:/projects/cross-window", "mission-one", () => new Promise<void>((resolve) => {
    release = resolve;
  }));
  while (!release) await new Promise<void>((resolve) => setImmediate(resolve));
  await assert.rejects(
    second.run("C:/projects/cross-window", "mission-two", async () => undefined),
    /Another VS Code window/,
  );
  release();
  await held;
});

test("a live child process blocks another window and its dead owner is recovered", async () => {
  const project = `C:/projects/cross-process-${process.pid}-${Date.now()}`;
  const digest = createHash("sha256").update(workspaceIdentity(project)).digest("hex");
  const lock = path.join(os.tmpdir(), "runtrol-project-integration-leases", digest);
  const child = spawn(process.execPath, [
    "-e",
    [
      "const fs = require('node:fs');",
      "const path = require('node:path');",
      "const lock = process.argv[1];",
      "fs.mkdirSync(lock, { recursive: false });",
      "fs.mkdirSync(path.join(lock, `pid-${process.pid}`));",
      "process.stdout.write('ready\\n');",
      "process.stdin.resume();",
    ].join(""),
    lock,
  ], { stdio: ["pipe", "pipe", "pipe"] });

  try {
    assert.ok(child.stdout);
    await waitForLine(child.stdout, "ready");
    await assert.rejects(
      new MissionProjectLeases().run(project, "parent-mission", async () => undefined),
      /Another VS Code window/,
    );

    child.stdin?.end();
    const [exitCode] = await once(child, "exit");
    assert.equal(exitCode, 0);
    await new MissionProjectLeases().run(project, "recovered-mission", async () => undefined);
  } finally {
    if (child.exitCode === null) {
      child.kill();
      await once(child, "exit");
    }
  }
});

async function waitForLine(stream: NodeJS.ReadableStream, expected: string): Promise<void> {
  let output = "";
  for await (const chunk of stream) {
    output += String(chunk);
    if (output.split(/\r?\n/).includes(expected)) return;
  }
  throw new Error(`child process exited before writing ${expected}`);
}
