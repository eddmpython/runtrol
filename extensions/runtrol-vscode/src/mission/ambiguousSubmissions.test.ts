import assert from "node:assert/strict";
import test from "node:test";

import { AmbiguousSubmissions } from "./ambiguousSubmissions";

test("restored ambiguity stays visible and writes stable Task identities only", async () => {
  const writes: string[][] = [];
  const submissions = new AmbiguousSubmissions(["task-b"], async (taskIds) => {
    writes.push([...taskIds]);
  });

  assert.equal(submissions.current().has("task-b"), true);
  await submissions.mark("task-a");
  assert.deepEqual(writes, [["task-a", "task-b"]]);
  assert.deepEqual([...submissions.current()].sort(), ["task-a", "task-b"]);
});

test("parallel acknowledgements serialize without losing the other marker", async () => {
  const writes: string[][] = [];
  const submissions = new AmbiguousSubmissions(["task-a", "task-b"], async (taskIds) => {
    await Promise.resolve();
    writes.push([...taskIds]);
  });

  await Promise.all([submissions.clear("task-a"), submissions.clear("task-b")]);
  assert.deepEqual(writes, [["task-b"], []]);
  assert.equal(submissions.current().size, 0);
});

test("a failed durable write keeps the conservative in-memory marker", async () => {
  const submissions = new AmbiguousSubmissions(["task-a"], () => Promise.reject(new Error("storage unavailable")));

  await assert.rejects(submissions.clear("task-a"), /storage unavailable/u);
  assert.equal(submissions.current().has("task-a"), true);
});

test("an idempotent update does not touch durable storage", async () => {
  let writes = 0;
  const submissions = new AmbiguousSubmissions(["task-a"], async () => {
    writes += 1;
  });

  await submissions.mark("task-a");
  assert.equal(writes, 0);
});
