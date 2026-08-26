import assert from "node:assert/strict";
import test from "node:test";

import { usageSnapshot, usageViewAction, type UsageViewSnapshot } from "./usageViewMessage";

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

test("a snapshot the host would actually send survives its own validator", () => {
  // The regression this exists for: the host and the document named the same field differently, the validator
  // silently dropped every message, and the panel drew nothing while looking merely empty. Building the value
  // from the host's own type is the point, because a hand-written literal would have kept passing.
  const sent: UsageViewSnapshot = {
    type: "snapshot",
    rows: [],
    setup: [{ providerId: "grok", name: "Grok", icon: "grok", state: "missing", detail: "Not installed", actionable: false }],
    error: null,
  };
  assert.deepEqual(usageSnapshot(JSON.parse(JSON.stringify(sent))), sent);
  assert.equal(usageSnapshot({ ...sent, setup: undefined }), null);
  assert.equal(usageSnapshot({ ...sent, rows: undefined }), null);
  assert.equal(usageSnapshot({ ...sent, error: 7 }), null);
  assert.equal(usageSnapshot("snapshot"), null);
});
