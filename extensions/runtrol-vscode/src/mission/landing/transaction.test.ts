import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLandingTransaction,
  createLandingDirectories,
  LandingTransactionError,
} from "./transaction";

test("a later write failure restores existing bytes and removes a newly created file", async () => {
  const files = new Map<string, Uint8Array>([["old.txt", Uint8Array.of(1)]]);
  let writes = 0;
  await assert.rejects(
    applyLandingTransaction([
      { path: "old.txt", sourceBytes: Uint8Array.of(2), targetBytes: Uint8Array.of(1) },
      { path: "new.txt", sourceBytes: Uint8Array.of(3), targetBytes: null },
    ], {
      beforeWrite: async () => undefined,
      read: async (path) => files.get(path) ?? null,
      write: async (path, bytes) => {
        writes += 1;
        if (writes === 2) throw new Error("injected write failure");
        files.set(path, bytes.slice());
      },
      remove: async (path) => {
        files.delete(path);
      },
    }),
    (error: unknown) => error instanceof LandingTransactionError
      && error.message === "injected write failure"
      && error.rollbackProblems.length === 0,
  );
  assert.deepEqual(files.get("old.txt"), Uint8Array.of(1));
  assert.equal(files.has("new.txt"), false);
});

test("post-write byte mismatch rolls back the whole bounded set", async () => {
  const files = new Map<string, Uint8Array>([["one.txt", Uint8Array.of(4)]]);
  let corruptNextRead = true;
  await assert.rejects(
    applyLandingTransaction([
      { path: "one.txt", sourceBytes: Uint8Array.of(5), targetBytes: Uint8Array.of(4) },
    ], {
      beforeWrite: async () => undefined,
      read: async (artifactPath) => {
        if (corruptNextRead) {
          corruptNextRead = false;
          return Uint8Array.of(9);
        }
        return files.get(artifactPath) ?? null;
      },
      write: async (path, bytes) => {
        files.set(path, bytes.slice());
      },
      remove: async (path) => {
        files.delete(path);
      },
    }),
    /written Artifact does not match/,
  );
  assert.deepEqual(files.get("one.txt"), Uint8Array.of(4));
});

test("rollback verifies a writer that reports success without restoring bytes", async () => {
  const files = new Map<string, Uint8Array>([["one.txt", Uint8Array.of(1)]]);
  let writes = 0;
  await assert.rejects(
    applyLandingTransaction([
      { path: "one.txt", sourceBytes: Uint8Array.of(2), targetBytes: Uint8Array.of(1) },
      { path: "two.txt", sourceBytes: Uint8Array.of(3), targetBytes: null },
    ], {
      beforeWrite: async () => undefined,
      read: async (artifactPath) => files.get(artifactPath) ?? null,
      write: async (artifactPath, bytes) => {
        writes += 1;
        if (writes === 2) throw new Error("injected write failure");
        if (writes === 3) return;
        files.set(artifactPath, bytes.slice());
      },
      remove: async (artifactPath) => {
        files.delete(artifactPath);
      },
    }),
    (error: unknown) => error instanceof LandingTransactionError
      && error.rollbackProblems.some((problem) => problem.includes("one.txt: restored bytes")),
  );
});

test("a compare-before-write failure restores only earlier writes", async () => {
  const files = new Map<string, Uint8Array>([
    ["one.txt", Uint8Array.of(1)],
    ["two.txt", Uint8Array.of(2)],
  ]);
  await assert.rejects(
    applyLandingTransaction([
      { path: "one.txt", sourceBytes: Uint8Array.of(3), targetBytes: Uint8Array.of(1) },
      { path: "two.txt", sourceBytes: Uint8Array.of(4), targetBytes: Uint8Array.of(2) },
    ], {
      beforeWrite: async (entry) => {
        if (entry.path === "two.txt") throw new Error("target changed before write");
      },
      read: async (artifactPath) => files.get(artifactPath) ?? null,
      write: async (artifactPath, bytes) => {
        files.set(artifactPath, bytes.slice());
      },
      remove: async (artifactPath) => {
        files.delete(artifactPath);
      },
    }),
    /target changed before write/,
  );
  assert.deepEqual(files.get("one.txt"), Uint8Array.of(1));
  assert.deepEqual(files.get("two.txt"), Uint8Array.of(2));
});

test("partial directory creation is removed and verified when a later creation fails", async () => {
  const directories = new Set<string>();
  await assert.rejects(
    createLandingDirectories(["one", "one/two"], {
      ensure: async (directory) => {
        if (directory === "one/two") throw new Error("injected directory failure");
        directories.add(directory);
        return true;
      },
      exists: async (directory) => directories.has(directory),
      remove: async (directory) => {
        directories.delete(directory);
      },
    }),
    (error: unknown) => error instanceof LandingTransactionError
      && error.message === "injected directory failure"
      && error.rollbackProblems.length === 0,
  );
  assert.deepEqual([...directories], []);
});

test("partial directory cleanup cannot be reported as restored when removal lies", async () => {
  const directories = new Set<string>();
  await assert.rejects(
    createLandingDirectories(["one", "one/two"], {
      ensure: async (directory) => {
        if (directory === "one/two") throw new Error("injected directory failure");
        directories.add(directory);
        return true;
      },
      exists: async (directory) => directories.has(directory),
      remove: async () => undefined,
    }),
    (error: unknown) => error instanceof LandingTransactionError
      && error.rollbackProblems.some((problem) => problem.includes("directory still exists")),
  );
});
