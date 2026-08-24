import assert from "node:assert/strict";
import test from "node:test";

import { usageViewAction } from "./usageViewMessage";

test("the usage view accepts only its bounded action vocabulary", () => {
  assert.deepEqual(usageViewAction({ type: "ready" }), { type: "ready" });
  assert.deepEqual(usageViewAction({ type: "discover" }), { type: "discover" });
  assert.deepEqual(usageViewAction({ type: "fix", providerId: "codex" }), {
    type: "fix",
    providerId: "codex",
  });
  assert.equal(usageViewAction({ type: "fix", providerId: "" }), null);
  assert.equal(usageViewAction({ type: "fix", providerId: "x".repeat(257) }), null);
  assert.equal(usageViewAction({ type: "delete", providerId: "codex" }), null);
  assert.equal(usageViewAction("fix"), null);
});
