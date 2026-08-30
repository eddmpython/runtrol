import assert from "node:assert/strict";
import test from "node:test";

import { BoundedDedupe } from "./boundedDedupe";

test("admits a value once while it remains retained", () => {
  const dedupe = new BoundedDedupe<string>(2);

  assert.equal(dedupe.remember("runtime unavailable"), true);
  assert.equal(dedupe.remember("runtime unavailable"), false);
});

test("evicts in insertion order without refreshing a duplicate", () => {
  const dedupe = new BoundedDedupe<string>(2);

  assert.equal(dedupe.remember("first"), true);
  assert.equal(dedupe.remember("second"), true);
  assert.equal(dedupe.remember("first"), false);
  assert.equal(dedupe.remember("third"), true);
  assert.equal(dedupe.remember("first"), true, "the oldest value was evicted");
  assert.equal(dedupe.remember("third"), false, "the newest retained value remains deduplicated");
});

test("independent owners never suppress one another", () => {
  const runtimeWarnings = new BoundedDedupe<string>(1);
  const isolatedWorkspaces = new BoundedDedupe<string>(1);

  assert.equal(runtimeWarnings.remember("shared identity"), true);
  assert.equal(isolatedWorkspaces.remember("shared identity"), true);
  assert.equal(runtimeWarnings.remember("shared identity"), false);
  assert.equal(isolatedWorkspaces.remember("shared identity"), false);
});

test("rejects capacities that cannot form a fixed positive bound", () => {
  assert.throws(() => new BoundedDedupe(0), RangeError);
  assert.throws(() => new BoundedDedupe(1.5), RangeError);
  assert.throws(() => new BoundedDedupe(Number.MAX_VALUE), RangeError);
});
