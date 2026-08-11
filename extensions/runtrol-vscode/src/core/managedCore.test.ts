import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { materializeManagedCore } from "./managedCore";

test("a bundled Core is installed once at a stable path and replaced by content", async () => {
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
    const firstIdentity = await stat(first.executable);

    const unchanged = await materializeManagedCore(firstSource, storage);
    assert.equal(unchanged.executable, first.executable);
    assert.equal(unchanged.digest, first.digest);
    assert.equal(unchanged.replaced, false);
    assert.equal((await stat(unchanged.executable)).ino, firstIdentity.ino);

    const upgraded = await materializeManagedCore(secondSource, storage);
    assert.equal(upgraded.executable, first.executable);
    assert.notEqual(upgraded.digest, first.digest);
    assert.equal(upgraded.replaced, true);
    assert.equal((await readFile(upgraded.executable, "utf8")), "second Core bytes");

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
    assert.equal(repaired.replaced, true);
    assert.equal((await readFile(repaired.executable, "utf8")), "trusted Core bytes");
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
