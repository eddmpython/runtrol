import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import { mkdir, mkdtemp, rename, rm, writeFile } from "node:fs/promises";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";

import { RuntimeLocatorError, type RuntimeGenerationSnapshot } from "../src/index.js";
import type { RuntimeGeneration, RuntimeLocatorRecord } from "../src/generated/protocol.js";
import { runtimeLocatorAt } from "../src/testing.js";
import { makeOwnerOnly } from "./locatorFixture.js";

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 10_000;
  while (!check()) {
    assert.ok(Date.now() < deadline, "locator observation exceeded the fixture deadline");
    await delay(10);
  }
}

type MutableGeneration = { -readonly [Key in keyof RuntimeGeneration]: RuntimeGeneration[Key] };
type MutableRecord = Omit<RuntimeLocatorRecord, "generations"> & { generations: MutableGeneration[] };

function generation(home: string, letter: string, startedAtMs: number): MutableGeneration {
  return {
    digest: letter.repeat(64),
    endpointKind: process.platform === "win32" ? "namedPipe" : "unixSocket",
    endpoint: process.platform === "win32" ? `\\\\.\\pipe\\runtrol-runtime-${letter.repeat(16)}`
      : join(home, `runtrol-runtime-${letter.repeat(16)}.sock`),
    controlEndpoint: `control-${letter}`,
    runtimeVersion: "0.1.1", processId: process.pid, startedAtMs, liveSessions: 0, draining: false,
  };
}

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "runtrol-ts-locator-watch-"));
  const home = join(root, "home");
  const path = join(home, "runtime.locator.json");
  const record: MutableRecord = {
    schema: 2, instanceId: `rtm_${"1".repeat(32)}`, generations: [generation(home, "a", 1)],
  };
  try {
    await mkdir(home);
    await writeFile(path, JSON.stringify(record));
    await makeOwnerOnly(path);
    return { root, home, path, record };
  } catch (error) { await rm(root, { recursive: true, force: true }); throw error; }
}

function latest(snapshots: readonly RuntimeGenerationSnapshot[]): RuntimeGenerationSnapshot {
  const value = snapshots.at(-1);
  assert.ok(value);
  return value;
}

class Watcher extends EventEmitter implements fs.FSWatcher {
  public closed = false;
  public close(): void { this.closed = true; this.emit("close"); }
  public ref(): this { return this; }
  public unref(): this { return this; }
}

test("null filename events suppress count-only validation and retain routing changes", async (t) => {
  const f = await fixture();
  const handles: Watcher[] = [];
  let reads = 0;
  const originalOpen = fs.promises.open;
  const fakeWatch: typeof fs.watch = (_path: fs.PathLike, options?: unknown, listener?: unknown) => {
    const handle = new Watcher();
    const callback = typeof options === "function" ? options : listener;
    assert.equal(typeof callback, "function");
    handle.on("change", (event: fs.WatchEventType, name: string | null) => {
      if (typeof callback === "function") callback(event, name);
    });
    handles.push(handle);
    return handle;
  };
  t.mock.method(fs, "watch", fakeWatch);
  t.mock.method(fs.promises, "open", async (...args: Parameters<typeof originalOpen>) => {
    if (args[0] === f.path) reads += 1;
    return originalOpen(...args);
  });
  syncBuiltinESMExports();
  const snapshots: RuntimeGenerationSnapshot[] = [];
  const abort = new AbortController();
  const done = runtimeLocatorAt(f.path).watchGenerations((value) => { snapshots.push(value); }, { signal: abort.signal });
  try {
    await until(() => snapshots.length === 1);
    assert.equal(handles.length, 2);
    const baseline = reads;
    const first = f.record.generations[0];
    assert.ok(first);
    first.liveSessions = 8;
    await writeFile(f.path, JSON.stringify(f.record));
    handles[0]?.emit("change", "change", null);
    await until(() => reads > baseline);
    await delay(50);
    assert.equal(reads, baseline + 1, "a count-only event reads its hint without a second validated read");
    assert.equal(snapshots.length, 1);
    const peer = generation(f.home, "c", 3);
    peer.draining = true;
    f.record.generations.push(peer);
    await writeFile(f.path, JSON.stringify(f.record));
    handles[0]?.emit("change", "rename", null);
    await until(() => snapshots.length === 2);
    assert.equal(latest(snapshots).generations.length, 2);
    const stable = reads;
    await delay(80);
    assert.equal(reads, stable, "idle observation does not reread the locator");
  } finally {
    abort.abort(); await done;
    t.mock.restoreAll(); syncBuiltinESMExports();
    await rm(f.root, { recursive: true, force: true });
  }
  assert.ok(handles.every((handle) => handle.closed));
});

test("actual locator replacement retains peer incarnations and distinguishes same-digest restarts", async () => {
  const f = await fixture();
  const snapshots: RuntimeGenerationSnapshot[] = [];
  const abort = new AbortController();
  const done = runtimeLocatorAt(f.path).watchGenerations((value) => { snapshots.push(value); }, { signal: abort.signal });
  const failure = done.catch((error: unknown) => error);
  try {
    await until(() => snapshots.length > 0);
    const initial = latest(snapshots).currentRevision;
    assert.match(initial ?? "", /^[0-9a-f]{64}$/);
    const peer = generation(f.home, "c", 3);
    peer.draining = true;
    f.record.generations.push(peer);
    await writeFile(f.path, JSON.stringify(f.record));
    await until(() => latest(snapshots).generations.length === 2);
    assert.equal(latest(snapshots).currentRevision, initial);
    const peerBefore = latest(snapshots).generations.find((entry) => entry.digest === peer.digest)?.revision;
    peer.startedAtMs += 1;
    await writeFile(f.path, JSON.stringify(f.record));
    await until(() => latest(snapshots).generations.find((entry) => entry.digest === peer.digest)?.revision !== peerBefore);
    assert.equal(latest(snapshots).currentRevision, initial, "a peer restart retains the primary incarnation");

    const predecessor = f.record.generations[0];
    assert.ok(predecessor);
    predecessor.draining = true;
    const successor = generation(f.home, "b", 2);
    f.record.generations.push(successor);
    const temporary = join(f.home, "replacement.json");
    await writeFile(temporary, JSON.stringify(f.record));
    await makeOwnerOnly(temporary);
    await rename(temporary, f.path);
    await until(() => {
      const current = latest(snapshots).current;
      return current.state === "running" && current.locator.digest === successor.digest;
    });
    assert.equal(latest(snapshots).generations.find((entry) => entry.digest === predecessor.digest)?.revision, initial);
    const beforeRestart = latest(snapshots).currentRevision;
    successor.startedAtMs += 1;
    await writeFile(f.path, JSON.stringify(f.record));
    await until(() => latest(snapshots).currentRevision !== beforeRestart);
    const current = latest(snapshots).current;
    assert.equal(current.state, "running");
    if (current.state === "running") {
      assert.equal(current.locator.digest, successor.digest);
      assert.equal(current.locator.revision, latest(snapshots).currentRevision);
    }
  } finally {
    abort.abort(); await done;
    await rm(f.root, { recursive: true, force: true });
  }
  assert.equal(await failure, undefined);
});

for (const operation of ["rename", "delete"] as const) {
  test(`actual home ${operation} ends observation and releases watcher handles`, async () => {
    const f = await fixture();
    const snapshots: RuntimeGenerationSnapshot[] = [];
    const abort = new AbortController();
    const done = runtimeLocatorAt(f.path).watchGenerations((value) => { snapshots.push(value); }, { signal: abort.signal });
    const result = done.then(() => undefined, (error: unknown) => error);
    try {
      await until(() => snapshots.length > 0);
      if (operation === "rename") {
        await rename(f.home, join(f.root, "moved"));
        await mkdir(f.home);
      } else await rm(f.home, { recursive: true });
      const error = await Promise.race([result, delay(10_000, "deadline", { ref: false })]);
      assert.ok(error instanceof Error, "replaced home must fail without a retry timer");
      assert.match(error.message, /ENOENT|directory|watcher|EPERM/);
    } finally {
      abort.abort(); await result;
      await rm(f.root, { recursive: true, force: true });
    }
  });
}

test("malformed and oversized locator changes fail closed without publication", async () => {
  const f = await fixture();
  try {
    for (const payload of ["{broken", "x".repeat(16_385)]) {
      await writeFile(f.path, payload);
      await assert.rejects(
        runtimeLocatorAt(f.path).watchGenerations(() => assert.fail("unvalidated publication")),
        (error: unknown) => error instanceof RuntimeLocatorError,
      );
    }
  } finally { await rm(f.root, { recursive: true, force: true }); }
});

test("validated reads enforce the byte ceiling after metadata and assemble partial reads", async (t) => {
  const f = await fixture();
  const bytes = Buffer.from(JSON.stringify(f.record));
  const originalOpen = fs.promises.open;
  let mode: "oversize" | "partial" = "oversize";
  let largest = 0;
  let closed = 0;
  t.mock.method(fs.promises, "open", async (...args: Parameters<typeof originalOpen>) => {
    const file = await originalOpen(...args);
    if (args[0] !== f.path) return file;
    const originalClose = file.close.bind(file);
    const read = (async (buffer: Buffer, offset: number, length: number, position: number) => {
      largest = Math.max(largest, buffer.length);
      if (mode === "oversize") {
        buffer.fill(120, offset, offset + length);
        return { bytesRead: length, buffer };
      }
      const count = Math.max(0, Math.min(length, 7, bytes.length - position));
      bytes.copy(buffer, offset, position, position + count);
      return { bytesRead: count, buffer };
    }) as typeof file.read;
    t.mock.method(file, "read", read);
    t.mock.method(file, "close", async () => { closed += 1; await originalClose(); });
    return file;
  });
  syncBuiltinESMExports();
  try {
    await assert.rejects(runtimeLocatorAt(f.path).inspect(), /byte limit/);
    assert.equal(largest, 16_385);
    assert.equal(closed, 1);
    mode = "partial";
    const state = await runtimeLocatorAt(f.path).inspect();
    assert.equal(state.state, "running");
    assert.equal(largest, 16_385);
    assert.equal(closed, 2);
  } finally {
    t.mock.restoreAll(); syncBuiltinESMExports();
    await rm(f.root, { recursive: true, force: true });
  }
});
