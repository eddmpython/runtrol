import assert from "node:assert/strict";
import { test } from "node:test";

import { checkCoreCurrency } from "./coreCurrency";
import type { CoreClient } from "./core/client";

function client(announced: string | null): CoreClient {
  return {
    ensureRuntime: async () => undefined,
    announcedBuildDigest: async () => announced,
  } as unknown as CoreClient;
}

test("the installed build answering is current", async () => {
  const outcome = await checkCoreCurrency(client("a".repeat(64)), async () => "a".repeat(64));
  assert.deepEqual(outcome, { state: "current" });
});

test("no managed digest means somebody else's build and nothing to compare", async () => {
  const outcome = await checkCoreCurrency(client("b".repeat(64)), async () => null);
  assert.deepEqual(outcome, { state: "current" });
});

test("another build answering is foreign, with what it announced", async () => {
  const outcome = await checkCoreCurrency(client("c".repeat(64)), async () => "d".repeat(64));
  assert.deepEqual(outcome, { state: "foreign", announced: "c".repeat(64) });
});

test("a daemon too old to announce a digest is foreign too", async () => {
  const outcome = await checkCoreCurrency(client(null), async () => "e".repeat(64));
  assert.deepEqual(outcome, { state: "foreign", announced: null });
});
