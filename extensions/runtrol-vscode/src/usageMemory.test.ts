import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderUsageGauge } from "./runtimeTypes";
import { rememberedUsage, rememberUsage, writeRememberedUsageNow } from "./usageMemory";

const NOW = Date.parse("2026-08-27T12:00:00Z");

/// The smallest memento the module uses: get and update over a map.
function memento() {
  const held = new Map<string, unknown>();
  return {
    get: <T>(key: string) => held.get(key) as T | undefined,
    update: async (key: string, value: unknown) => {
      held.set(key, value);
    },
    keys: () => [...held.keys()],
  } as never;
}

function gauge(overrides: Partial<ProviderUsageGauge>): ProviderUsageGauge {
  return {
    providerId: "claude",
    reached: false,
    atMs: NOW - 60_000,
    windows: [{ id: "five_hour", usedPercent: 21, windowMinutes: 300, resetsAtMs: NOW + 3_600_000 }],
    ...overrides,
  } as ProviderUsageGauge;
}

test("a strip written a minute ago is drawn again", async () => {
  // The whole point: a window that opens right after another closes shows bars instead of "Checking usage"
  // for the ten-odd seconds it takes to ask three services.
  const store = memento();
  rememberUsage(store, [gauge({})]);
  await writeRememberedUsageNow();
  const restored = rememberedUsage(store, NOW);
  assert.equal(restored.length, 1);
  assert.equal(restored[0]?.windows?.[0]?.usedPercent, 21);
});

test("a window whose reset has passed is not drawn again", async () => {
  // The reading is not stale here, it is wrong, and wrong in the direction that tells somebody they are out
  // of room on a window they have just been given fresh.
  const store = memento();
  rememberUsage(store, [gauge({
    windows: [{ id: "seven_day", usedPercent: 100, windowMinutes: 10_080, resetsAtMs: NOW - 1 }],
  })]);
  await writeRememberedUsageNow();
  assert.deepEqual(rememberedUsage(store, NOW), []);
});

test("a window with no stated reset survives on the age bound alone", async () => {
  const store = memento();
  rememberUsage(store, [gauge({ windows: [{ id: "billing_period", usedPercent: 5 }] })]);
  await writeRememberedUsageNow();
  assert.equal(rememberedUsage(store, NOW).length, 1);
});

test("a strip from yesterday is not drawn at all", async () => {
  const store = memento();
  rememberUsage(store, [gauge({ atMs: NOW - 24 * 3_600_000 })]);
  await writeRememberedUsageNow();
  assert.deepEqual(rememberedUsage(store, NOW), []);
});

test("a reading stamped in the future is refused", async () => {
  // A clock that moved backwards. Its reading is about a moment this window cannot reason about.
  const store = memento();
  rememberUsage(store, [gauge({ atMs: NOW + 60_000 })]);
  await writeRememberedUsageNow();
  assert.deepEqual(rememberedUsage(store, NOW), []);
});

test("a strip written by an older build is discarded rather than drawn", async () => {
  const store = memento();
  await (store as unknown as { update(key: string, value: unknown): Promise<void> }).update(
    "runtrol.usageMemory.v1",
    { gauges: [{ providerId: "claude", primary: { usedPercent: 5 } }] },
  );
  assert.deepEqual(rememberedUsage(store, NOW), []);
  const wrong = memento();
  await (wrong as unknown as { update(key: string, value: unknown): Promise<void> }).update(
    "runtrol.usageMemory.v1",
    { gauges: "everything" },
  );
  assert.deepEqual(rememberedUsage(wrong, NOW), []);
});
