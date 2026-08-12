import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { SelectionStore } from "./selectionStore";

test("one bounded scalar selection survives a new store instance", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-selection-"));
  try {
    const first = new SelectionStore(root);
    assert.equal(await first.load(), null);
    await first.save("019ff230-052f-7680-ab86-a1771017dcfb");

    const reloaded = new SelectionStore(root);
    assert.equal(await reloaded.load(), "019ff230-052f-7680-ab86-a1771017dcfb");
    await reloaded.clear();
    assert.equal(await reloaded.load(), null);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("oversized, malformed, and control-bearing values never become a selection", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-selection-invalid-"));
  try {
    const store = new SelectionStore(root);
    await writeFile(path.join(root, "selected-session.json"), "x".repeat(257), "utf8");
    assert.equal(await store.load(), null);
    await writeFile(path.join(root, "selected-session.json"), "{", "utf8");
    assert.equal(await store.load(), null);
    await assert.rejects(() => store.save("bad\nsession"), /identifier is invalid/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("transient selection writes retry within one bounded window", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-selection-retry-"));
  let attempts = 0;
  try {
    const store = new SelectionStore(root, async (file, contents) => {
      attempts += 1;
      if (attempts < 3) {
        throw Object.assign(new Error("injected lock"), { code: "EPERM" });
      }
      await writeFile(file, contents, { encoding: "utf8", mode: 0o600 });
    });
    await store.save("019ff230-052f-7680-ab86-a1771017dcfb");
    assert.equal(attempts, 3);
    assert.equal(await store.load(), "019ff230-052f-7680-ab86-a1771017dcfb");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("permanent selection write errors fail without retrying", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-selection-permanent-"));
  let attempts = 0;
  try {
    const store = new SelectionStore(root, async () => {
      attempts += 1;
      throw Object.assign(new Error("injected permanent failure"), { code: "EIO" });
    });
    await assert.rejects(() => store.save("019ff230-052f-7680-ab86-a1771017dcfb"), /permanent failure/);
    assert.equal(attempts, 1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
