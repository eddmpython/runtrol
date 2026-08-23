import assert from "node:assert/strict";
import test from "node:test";

import { WatchLifecycleGate } from "./watchLifecycleGate";

test("watch handshakes enter one at a time", async () => {
  const gate = new WatchLifecycleGate();
  const first = await gate.acquire("foreground", new AbortController().signal);
  assert.ok(first);

  let secondEntered = false;
  const secondOpening = gate.acquire("foreground", new AbortController().signal).then((release) => {
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

test("foreground openings pass queued background work", async () => {
  const gate = new WatchLifecycleGate();
  const first = await gate.acquire("background", new AbortController().signal);
  assert.ok(first);

  let backgroundEntered = false;
  const backgroundOpening = gate.acquire("background", new AbortController().signal).then((release) => {
    backgroundEntered = true;
    return release;
  });
  const foregroundOpening = gate.acquire("foreground", new AbortController().signal);
  first();

  const foreground = await foregroundOpening;
  assert.ok(foreground);
  assert.equal(backgroundEntered, false);
  foreground();

  const background = await backgroundOpening;
  assert.ok(background);
  background();
});

test("an aborted waiter leaves no dead place in front of the next watch", async () => {
  const gate = new WatchLifecycleGate();
  const first = await gate.acquire("foreground", new AbortController().signal);
  assert.ok(first);

  const cancelled = new AbortController();
  const cancelledOpening = gate.acquire("foreground", cancelled.signal);
  cancelled.abort();
  assert.equal(await cancelledOpening, null);

  const nextOpening = gate.acquire("foreground", new AbortController().signal);
  first();
  const next = await nextOpening;
  assert.ok(next);
  next();
});
