import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import {
  EXTENSION_IDENTIFIER,
  defaultGlobalStorages,
  managedDaemons,
  plan,
  readRecord,
  stateRootOf,
  standaloneRootOf,
  storageFromDaemons,
  stopManagedDaemons,
} from "./uninstall";

const home = path.resolve("C:\\Users\\someone");
const env = { LOCALAPPDATA: path.join(home, "AppData", "Local"), APPDATA: path.join(home, "AppData", "Roaming") };

test("the record names the storage the hook removes, and only a record of the exact shape counts", () => {
  const storage = path.join(env.APPDATA, "Code", "User", "globalStorage", EXTENSION_IDENTIFIER);
  const good = JSON.stringify({ schema: 1, globalStorage: storage });
  assert.deepEqual(readRecord("hook", () => good), { schema: 1, globalStorage: storage });
  for (const bad of [
    "not json",
    JSON.stringify({ schema: 2, globalStorage: storage }),
    JSON.stringify({ schema: 1, globalStorage: "relative/storage" }),
    JSON.stringify({ schema: 1, globalStorage: path.join(env.APPDATA, "Code", "User") }),
  ]) {
    assert.equal(readRecord("hook", () => bad), null, bad);
  }
  assert.equal(readRecord("hook", () => { throw new Error("ENOENT"); }), null);
});

test("without a record the hook removes only a default storage location that exists", () => {
  const candidates = defaultGlobalStorages(env, "win32", home);
  assert.equal(candidates.length, 2);
  const decided = plan(null, env, (file) => file === candidates[1], "win32", home);
  assert.equal(decided.globalStorage, candidates[1]);
  assert.equal(plan(null, env, () => false, "win32", home).globalStorage, null);
});

test("the state root follows RUNTROL_HOME first and stays when a standalone Runtime shares it", () => {
  assert.equal(stateRootOf({ ...env, RUNTROL_HOME: "D:\\elsewhere\\runtrol" }, "win32", home), "D:\\elsewhere\\runtrol");
  assert.equal(stateRootOf(env, "win32", home), path.join(env.LOCALAPPDATA, "runtrol"));
  const standalone = standaloneRootOf(env, "win32", home);
  assert.equal(plan(null, env, (file) => file === standalone, "win32", home).removeStateRoot, false);
  assert.equal(plan(null, env, () => false, "win32", home).removeStateRoot, true);
});

test("only daemons running from the managed Core directory are Studio's to stop", () => {
  const managedCore = path.join(env.APPDATA, "Code", "User", "globalStorage", EXTENSION_IDENTIFIER, "core");
  const ours = { pid: 10, executable: path.join(managedCore, "runtrol-0123456789abcdef.exe") };
  const standalone = { pid: 11, executable: path.join(env.LOCALAPPDATA, "RuntrolRuntime", "0.1.1", "runtrol.exe") };
  const lookalike = { pid: 12, executable: path.join(managedCore + "-else", "runtrol.exe") };
  assert.deepEqual(managedDaemons(managedCore, [ours, standalone, lookalike]), [ours]);
  assert.deepEqual(managedDaemons(managedCore.toUpperCase(), [ours]), [ours]);
});

test("a running Studio daemon proves its global storage from the locator and the process list", () => {
  const storage = path.join(env.APPDATA, "Code", "User", "globalStorage", EXTENSION_IDENTIFIER);
  const image = path.join(storage, "core", "runtrol-0123456789abcdef.exe");
  const locator = JSON.stringify({ schema: 2, generations: [{ processId: 40 }, { processId: 41 }] });
  const processes = [
    { pid: 39, executable: image },
    { pid: 41, executable: path.join(env.LOCALAPPDATA, "RuntrolRuntime", "0.1.1", "runtrol.exe") },
    { pid: 40, executable: image.toUpperCase() },
  ];
  assert.equal(storageFromDaemons(locator, processes)?.toLowerCase(), storage.toLowerCase());
  assert.equal(storageFromDaemons(locator, processes.slice(0, 2)), null, "a pid outside the locator proves nothing");
  assert.equal(storageFromDaemons("not json", processes), null);
  assert.equal(storageFromDaemons(null, processes), null);
  const decided = plan(null, env, () => false, "win32", home, storage);
  assert.equal(decided.globalStorage, storage);
});

test("shutdown stops published generations newest first and keeps the keeper outside stop requests", () => {
  const core = path.join(home, "managed", "core");
  const older = { pid: 11, executable: path.join(core, "older.exe") };
  const newer = { pid: 21, executable: path.join(core, "newer.exe") };
  const keeper = { pid: 22, executable: newer.executable };
  let active = [older, newer, keeper];
  const stopped: string[] = [];
  const olderDigest = "a".repeat(64);
  const newerDigest = "b".repeat(64);
  stopManagedDaemons(core, JSON.stringify({ generations: [
    { processId: older.pid, startedAtMs: 100, digest: olderDigest },
    { processId: newer.pid, startedAtMs: 200, digest: newerDigest },
  ] }), {
    list: () => active,
    stop: (digest) => {
      stopped.push(digest);
      active = active.filter((entry) => entry.executable !== (digest === newerDigest ? newer.executable : older.executable));
    },
  });
  assert.deepEqual(stopped, [newerDigest, olderDigest]);
});

test("an unconfirmed keeper prevents removal instead of being killed to release the image", () => {
  const core = path.join(home, "managed", "core");
  const keeper = { pid: 22, executable: path.join(core, "newer.exe") };
  assert.throws(() => stopManagedDaemons(core, JSON.stringify({ generations: [] }), {
    list: () => [keeper],
    stop: () => assert.fail("no live Runtime was published"),
  }), /supervision is still active/u);
  assert.throws(() => stopManagedDaemons(core, null, {
    list: () => [keeper],
    stop: () => assert.fail("no locator can authorize a Runtime request"),
  }), /no readable locator/u);
});

test("failed process completion is surfaced before any remaining process is considered gone", () => {
  const core = path.join(home, "managed", "core");
  const runtime = { pid: 21, executable: path.join(core, "newer.exe") };
  assert.throws(() => stopManagedDaemons(core, JSON.stringify({ generations: [{ processId: 21, startedAtMs: 100, digest: "a".repeat(64) }] }), {
    list: () => [runtime],
    stop: () => { throw new Error("keeper completion is unconfirmed"); },
  }), /completion is unconfirmed/u);
});
