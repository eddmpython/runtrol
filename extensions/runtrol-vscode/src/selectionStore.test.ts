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
