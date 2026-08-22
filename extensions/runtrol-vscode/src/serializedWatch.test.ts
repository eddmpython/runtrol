import assert from "node:assert/strict";
import test from "node:test";

import { SerializedWatch } from "./serializedWatch";

test("a replacement waits for the aborted watch to finish", async () => {
  const watch = new SerializedWatch();
  const started: string[] = [];
  let finishFirst = () => {};
  const firstFinished = new Promise<void>((resolve) => {
    finishFirst = resolve;
  });

  watch.start("conversation", async (_signal, ready) => {
    started.push("first");
    ready();
    await firstFinished;
  });
  await watch.settled();
  watch.pause();
  watch.start("conversation", async (_signal, ready) => {
    started.push("second");
    ready();
  });

  await Promise.resolve();
  assert.deepEqual(started, ["first"]);
  finishFirst();
  await watch.settled();
  assert.deepEqual(started, ["first", "second"]);
});

test("pausing a queued replacement prevents it from opening", async () => {
  const watch = new SerializedWatch();
  const started: string[] = [];
  let finishFirst = () => {};
  const firstFinished = new Promise<void>((resolve) => {
    finishFirst = resolve;
  });

  watch.start("first", async (_signal, ready) => {
    started.push("first");
    ready();
    await firstFinished;
  });
  await watch.settled();
  watch.pause();
  watch.start("second", async (_signal, ready) => {
    started.push("second");
    ready();
  });
  watch.pause();
  finishFirst();
  await Promise.resolve();
  await Promise.resolve();

  assert.deepEqual(started, ["first"]);
  assert.equal(watch.requested, false);
});

test("repeated starts for the current request do not duplicate the watch", async () => {
  const watch = new SerializedWatch();
  let starts = 0;
  let finish = () => {};
  const finished = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const run = async (_signal: AbortSignal, ready: () => void) => {
    starts += 1;
    ready();
    await finished;
  };

  watch.start("conversation", run);
  watch.start("conversation", run);
  await watch.settled();
  assert.equal(starts, 1);

  watch.dispose();
  finish();
});
