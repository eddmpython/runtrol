import assert from "node:assert/strict";
import test from "node:test";

import { ActivityWatchGate } from "./activityWatchGate";

test("background watch handshakes enter one at a time", async () => {
  const gate = new ActivityWatchGate();
  const first = await gate.acquire(new AbortController().signal);
  assert.ok(first);

  let secondEntered = false;
  const secondOpening = gate.acquire(new AbortController().signal).then((release) => {
    secondEntered = true;
    return release;
  });
  await Promise.resolve();
  assert.equal(secondEntered, false);

  first();
  const second = await secondOpening;
  assert.ok(second);
  second();
});

test("an aborted waiter leaves no dead place in front of the next watch", async () => {
  const gate = new ActivityWatchGate();
  const first = await gate.acquire(new AbortController().signal);
  assert.ok(first);

  const cancelled = new AbortController();
  const cancelledOpening = gate.acquire(cancelled.signal);
  cancelled.abort();
  assert.equal(await cancelledOpening, null);

  const nextOpening = gate.acquire(new AbortController().signal);
  first();
  const next = await nextOpening;
  assert.ok(next);
  next();
});

test("releasing a permit twice cannot admit two watches", async () => {
  const gate = new ActivityWatchGate();
  const first = await gate.acquire(new AbortController().signal);
  assert.ok(first);
  const secondOpening = gate.acquire(new AbortController().signal);
  const thirdOpening = gate.acquire(new AbortController().signal);

  first();
  first();
  const second = await secondOpening;
  assert.ok(second);
  let thirdEntered = false;
  void thirdOpening.then(() => { thirdEntered = true; });
  await Promise.resolve();
  assert.equal(thirdEntered, false);
  second();
  const third = await thirdOpening;
  assert.ok(third);
  third();
});
