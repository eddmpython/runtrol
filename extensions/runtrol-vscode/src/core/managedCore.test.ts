import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { managedCoreDirectory, materializeManagedCore } from "./managedCore";

/// The images in the managed directory, without the digest memory that lives beside them.
async function images(directory: string): Promise<string[]> {
  return (await readdir(directory)).filter((name) => name !== "digests.json").sort();
}

test("the same bundled Core is recognised by identity without being hashed again", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-managed-core-memory-"));
  const source = path.join(temporary, "source-core");
  const storage = path.join(temporary, "storage");
  try {
    await writeFile(source, "remembered Core bytes", "utf8");
    const first = await materializeManagedCore(source, storage);
    const memory = JSON.parse(await readFile(path.join(storage, "core", "digests.json"), "utf8")) as Record<string, { digest: string }>;
    assert.equal(memory[source]?.digest, first.digest, "the bundled source is remembered by identity");
    assert.equal(memory[first.executable]?.digest, first.digest, "the installed image is remembered too");
    const again = await materializeManagedCore(source, storage);
    assert.equal(again.executable, first.executable);
    assert.equal(again.replaced, false);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("a bundled Core is installed once under a content-named path and a new build gets its own file", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-managed-core-"));
  const firstSource = path.join(temporary, "first-core");
  const secondSource = path.join(temporary, "second-core");
  const storage = path.join(temporary, "storage");
  try {
    await writeFile(firstSource, "first Core bytes", "utf8");
    await writeFile(secondSource, "second Core bytes", "utf8");

    const first = await materializeManagedCore(firstSource, storage);
    assert.equal(first.replaced, false);
    assert.equal((await readFile(first.executable, "utf8")), "first Core bytes");
    assert.ok(path.basename(first.executable).startsWith(`runtrol-${first.digest.slice(0, 16)}`));
    const firstIdentity = await stat(first.executable);

    const unchanged = await materializeManagedCore(firstSource, storage);
    assert.equal(unchanged.executable, first.executable);
    assert.equal(unchanged.digest, first.digest);
    assert.equal(unchanged.replaced, false);
    assert.equal((await stat(unchanged.executable)).ino, firstIdentity.ino, "the same build is never rewritten");

    // The new build lands beside the old one, never over it: the file a running daemon was started from
    // is not touched, which is what made replacement fail with EPERM on Windows.
    const upgraded = await materializeManagedCore(secondSource, storage);
    assert.notEqual(upgraded.executable, first.executable);
    assert.notEqual(upgraded.digest, first.digest);
    assert.equal(upgraded.replaced, true);
    assert.equal((await readFile(upgraded.executable, "utf8")), "second Core bytes");
    assert.equal(managedCoreDirectory(upgraded.executable), managedCoreDirectory(first.executable));
    const remaining = await images(managedCoreDirectory(upgraded.executable));
    assert.deepEqual(remaining, [path.basename(upgraded.executable)], "the previous image is removed once nothing runs it");

    const rolledBack = await materializeManagedCore(firstSource, storage);
    assert.equal(rolledBack.executable, first.executable);
    assert.equal(rolledBack.digest, first.digest);
    assert.equal(rolledBack.replaced, true);
    assert.equal((await readFile(rolledBack.executable, "utf8")), "first Core bytes");
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("a corrupted managed Core is repaired from the bundled source", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-managed-core-corrupt-"));
  const source = path.join(temporary, "source-core");
  const storage = path.join(temporary, "storage");
  try {
    await writeFile(source, "trusted Core bytes", "utf8");
    const installed = await materializeManagedCore(source, storage);
    await writeFile(installed.executable, "corrupt", "utf8");

    const repaired = await materializeManagedCore(source, storage);
    assert.equal(repaired.executable, installed.executable);
    assert.equal((await readFile(repaired.executable, "utf8")), "trusted Core bytes");
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("the single-name image older extensions installed is cleared away", async () => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-managed-core-legacy-"));
  const source = path.join(temporary, "source-core");
  const storage = path.join(temporary, "storage");
  try {
    await writeFile(source, "current Core bytes", "utf8");
    const core = path.join(storage, "core");
    await (await import("node:fs/promises")).mkdir(core, { recursive: true });
    const legacyName = process.platform === "win32" ? "runtrol.exe" : "runtrol";
    await writeFile(path.join(core, legacyName), "old Core bytes", "utf8");
    await writeFile(path.join(core, `${legacyName}.inuse-${"a".repeat(64)}`), "old Core bytes", "utf8");

    const installed = await materializeManagedCore(source, storage);
    assert.equal(installed.replaced, true);
    assert.deepEqual(await images(core), [path.basename(installed.executable)]);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
