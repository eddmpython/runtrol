import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { writeAtomicLandingFile } from "./atomicFile";

test("a failed final guard leaves the target exact and removes the exclusive temporary file", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "runtrol-landing-atomic-"));
  const target = path.join(root, "artifact.txt");
  try {
    await writeFile(target, "before", "utf8");
    await assert.rejects(
      writeAtomicLandingFile(root, target, new TextEncoder().encode("after"), null, async () => {
        throw new Error("injected CAS failure");
      }),
      /injected CAS failure/,
    );
    assert.equal(await readFile(target, "utf8"), "before");
    assert.deepEqual((await readdir(root)).filter((entry) => entry.startsWith(".runtrol-landing-")), []);

    await writeAtomicLandingFile(root, target, new TextEncoder().encode("after"), null, async () => undefined);
    assert.equal(await readFile(target, "utf8"), "after");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
