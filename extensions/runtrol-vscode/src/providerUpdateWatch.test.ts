import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderUpdateLine } from "./protocol";
import { ProviderUpdateWatch } from "./providerUpdateWatch";

function line(overrides: Partial<ProviderUpdateLine>): ProviderUpdateLine {
  return {
    provider: "claude",
    state: "current",
    package: "@anthropic-ai/claude-code",
    installed: "2.1.251",
    target: null,
    rollback: null,
    why: null,
    ...overrides,
  };
}

test("an update is offered only when the Core confirms a newer release and an exact rollback", async () => {
  let answer: ProviderUpdateLine[] = [line({})];
  const watch = new ProviderUpdateWatch(async () => answer, 60_000);
  let changed = 0;
  watch.onDidChange(() => { changed += 1; });
  await watch.start();
  assert.equal(watch.installedFor("claude"), "2.1.251");
  assert.equal(watch.updateTargetFor("claude"), null);
  assert.equal(changed, 1);

  answer = [line({ state: "available", target: "2.1.252", rollback: null })];
  await watch.check();
  assert.equal(watch.updateTargetFor("claude"), null, "no rollback, no button: the Core would refuse the update");

  answer = [line({ state: "available", target: "2.1.252", rollback: "2.1.251" })];
  await watch.check();
  assert.equal(watch.updateTargetFor("claude"), "2.1.252");
  assert.equal(changed, 3);

  await watch.check();
  assert.equal(changed, 3, "the same answer again is not a change");
  watch.dispose();
});

test("a registry that does not answer keeps the previous answer", async () => {
  let fail = false;
  const watch = new ProviderUpdateWatch(async () => {
    if (fail) throw new Error("registry timeout");
    return [line({ state: "available", target: "2.1.252", rollback: "2.1.251" })];
  }, 60_000);
  await watch.start();
  assert.equal(watch.updateTargetFor("claude"), "2.1.252");
  fail = true;
  await watch.check();
  assert.equal(watch.updateTargetFor("claude"), "2.1.252");
  watch.dispose();
});

test("two calls while one inspection is running share it", async () => {
  let calls = 0;
  const watch = new ProviderUpdateWatch(async () => {
    calls += 1;
    await new Promise((resolve) => setTimeout(resolve, 5));
    return [line({})];
  }, 60_000);
  await Promise.all([watch.check(), watch.check(), watch.check()]);
  assert.equal(calls, 1);
  watch.dispose();
});
