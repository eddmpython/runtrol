import assert from "node:assert/strict";
import test from "node:test";

import { cancelQueued, pushQueued, queuedLabel, takeQueued } from "./queue";

test("messages queue in order and leave first in first out", () => {
  let queue: readonly string[] = [];
  for (const text of ["one", "two", "three"]) {
    const outcome = pushQueued(queue, text);
    assert.ok(outcome.accepted);
    queue = outcome.queue;
  }
  const first = takeQueued(queue);
  assert.equal(first.next, "one");
  const second = takeQueued(first.queue);
  assert.equal(second.next, "two");
  assert.deepEqual(second.queue, ["three"]);
  assert.equal(takeQueued([]).next, null);
});

test("the queue refuses emptiness, overflow, and oversize with a reason", () => {
  assert.equal(pushQueued([], "   ").accepted, false);
  const full = Array.from({ length: 8 }, (unused, index) => `m${index}`);
  const overflow = pushQueued(full, "ninth");
  assert.equal(overflow.accepted, false);
  assert.deepEqual(overflow.queue, full, "a refused push changes nothing");
  assert.equal(pushQueued([], "x".repeat(9000)).accepted, false);
});

test("cancelling removes exactly the pointed-at message and ignores nonsense indexes", () => {
  const queue = ["a", "b", "c"];
  assert.deepEqual(cancelQueued(queue, 1), ["a", "c"]);
  assert.deepEqual(cancelQueued(queue, -1), queue);
  assert.deepEqual(cancelQueued(queue, 3), queue);
  assert.deepEqual(cancelQueued(queue, 0.5), queue);
});

test("the strip label is one bounded line", () => {
  assert.equal(queuedLabel("  hello\n  world  "), "hello world");
  const long = queuedLabel("y".repeat(200));
  assert.equal(long.length, 80);
  assert.ok(long.endsWith("…"));
});
