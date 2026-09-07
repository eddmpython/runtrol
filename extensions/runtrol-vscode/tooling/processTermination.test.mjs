import assert from "node:assert/strict";
import { fork } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { terminateCapturedIdentities } from "./isolated-vscode.mjs";
import { normalizedExecutable, processRows } from "./process-identity.mjs";
import { terminateWindowsProcesses } from "./processTermination.mjs";

test("Windows termination pins the captured generation and refuses stale or unknown identities", {
  skip: process.platform !== "win32",
  timeout: 60_000,
}, async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-owned-exit-"));
  const file = path.join(root, "child.cjs");
  await writeFile(file, "process.on('message', value => { if (value === 'exit') process.exit(0); else process.send(value); });"
    + "process.send('ready'); setTimeout(() => process.exit(0), 50000);");
  const children = [];
  let identities = [];
  const start = async () => {
    const child = fork(file, [], { silent: true, windowsHide: true });
    const exited = once(child, "exit");
    children.push({ child, exited });
    assert.equal((await once(child, "message"))[0], "ready");
    return child;
  };
  const ping = async (child) => {
    const reply = once(child, "message");
    child.send("alive");
    assert.equal((await reply)[0], "alive");
  };
  try {
    const first = await start();
    const second = await start();
    identities = processRows().filter((row) => row.pid === first.pid || row.pid === second.pid);
    assert.equal(identities.length, 2);
    for (const identity of identities) {
      assert.ok(identity.startedAt > 0);
      assert.equal(normalizedExecutable(identity.executable), normalizedExecutable(process.execPath));
      assert.ok(identity.command.includes(file));
    }
    await writeFile(path.join(root, "ownership.json"), JSON.stringify(identities));
    const prior = identities.find((row) => row.pid === first.pid);
    const current = identities.find((row) => row.pid === second.pid);
    assert.notEqual(prior.startedAt, current.startedAt);

    // A stale capture points at a live different generation with the very same executable.
    await terminateCapturedIdentities([{ ...prior, pid: current.pid }]);
    await ping(second);
    await terminateCapturedIdentities([{ ...current, executable: path.join(root, "different.exe") }]);
    await ping(second);
    for (const startedAt of [0, null, undefined, NaN]) {
      // Validation covers the whole batch before the first otherwise valid child can be terminated.
      assert.throws(() => terminateWindowsProcesses([prior, { ...current, startedAt }], 0), /known PID, birth/u);
      await ping(first);
      await ping(second);
    }

    await terminateCapturedIdentities([prior]);
    await children[0].exited;
    await ping(second);
    await terminateCapturedIdentities([prior]);
    await terminateCapturedIdentities([current]);
    await children[1].exited;
  } finally {
    // Cooperative exit uses the already-connected child channel even when identity capture itself failed.
    for (const { child } of children) if (child.connected) child.send("exit");
    await Promise.all(children.map(({ exited }) => exited));
    const current = processRows();
    assert.ok(identities.every((identity) => !current.some((row) => row.pid === identity.pid && row.startedAt === identity.startedAt)));
    assert.equal(path.dirname(root), path.resolve(os.tmpdir()));
    await rm(root, { recursive: true });
  }
});
