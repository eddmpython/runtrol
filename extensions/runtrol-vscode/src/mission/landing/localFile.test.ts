import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { inspectSafeLocalFile, readExactLocalFile } from "./localFile";

test("the bounded reader rejects a known oversize file before allocating its body", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-landing-file-"));
  try {
    await writeFile(path.join(root, "artifact.txt"), "12345", "utf8");
    const file = await inspectSafeLocalFile(root, "artifact.txt", true);
    assert.ok(file);
    await assert.rejects(readExactLocalFile(file, 4, "Artifact"), /exceeds the Landing byte limit/);
    assert.deepEqual(await readExactLocalFile(file, 5, "Artifact"), new TextEncoder().encode("12345"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
