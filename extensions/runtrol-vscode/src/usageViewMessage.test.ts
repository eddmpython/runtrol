import assert from "node:assert/strict";
import test from "node:test";

import { usageViewAction } from "./usageViewMessage";

test("the usage view accepts only its bounded action vocabulary", () => {
  assert.deepEqual(usageViewAction({ type: "ready" }), { type: "ready" });
  assert.deepEqual(usageViewAction({ type: "setUp", providerId: "grok" }), {
    type: "setUp",
    providerId: "grok",
  });
  // The old catalogue action is gone, so the document cannot ask for it by name any more.
  assert.equal(usageViewAction({ type: "discover" }), null);
  assert.deepEqual(usageViewAction({ type: "fix", providerId: "codex" }), {
    type: "fix",
    providerId: "codex",
  });
  assert.equal(usageViewAction({ type: "fix", providerId: "" }), null);
  assert.equal(usageViewAction({ type: "fix", providerId: "x".repeat(257) }), null);
  assert.equal(usageViewAction({ type: "delete", providerId: "codex" }), null);
  assert.equal(usageViewAction("fix"), null);
});
