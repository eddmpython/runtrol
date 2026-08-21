import assert from "node:assert/strict";
import test from "node:test";

import { isViewAction } from "./viewActions";

test("accepts exactly the actions the dispatcher handles", () => {
  assert.ok(isViewAction({ type: "prompt", text: "hello" }));
  assert.ok(isViewAction({ type: "interrupt" }));
  assert.ok(isViewAction({ type: "openDiff", diff: { kind: "unified", path: "a.rs", text: "+x" } }));
  assert.ok(isViewAction({ type: "menuChoice", menu: "m1", choice: "3" }));
  assert.ok(isViewAction({ type: "menuChoice", menu: "m1", choice: null }));
  assert.equal(isViewAction({ type: "menuChoice", menu: "m1", choice: 3 }), false);
  assert.equal(isViewAction({ type: "openDiff", diff: { kind: "unified", path: "a.rs" } }), false);
  assert.ok(isViewAction({
    type: "answerApproval",
    approval: "a-1",
    option: 0,
    subjectDigest: [0, 255],
  }));
  assert.ok(isViewAction({ type: "switchModel", available: ["sonnet"] }));
  assert.ok(isViewAction({ type: "switchMode", available: ["plan"] }));
  assert.ok(isViewAction({ type: "switchEffort", model: "gpt-5" }));
  assert.ok(isViewAction({ type: "switchEffort", model: "" }), "an unknown model is refused later, honestly");
  assert.ok(isViewAction({ type: "pickProject" }));
  assert.ok(isViewAction({ type: "pickService" }));
  assert.ok(isViewAction({ type: "attach" }));
  assert.ok(isViewAction({ type: "removeAttachment", index: 0 }));
  assert.ok(isViewAction({ type: "mentionFile" }));
});

// Regression: these two names once validated without a dispatcher branch, so the dispatcher's
// interrupt fallback ran for them. An unknown or unhandled action must never become an interrupt.
test("refuses action names that have no dispatcher branch", () => {
  assert.equal(isViewAction({ type: "openWorkspace" }), false);
  assert.equal(isViewAction({ type: "close" }), false);
  assert.equal(isViewAction({ type: "startChat" }), false, "the draft tab replaced the start buttons");
  assert.equal(isViewAction({ type: "anythingElse" }), false);
});

test("refuses malformed payloads for known action names", () => {
  assert.equal(isViewAction({ type: "prompt" }), false);
  assert.equal(isViewAction({ type: "answerApproval", approval: "a", option: "0", subjectDigest: [] }), false);
  assert.equal(isViewAction({ type: "answerApproval", approval: "a", option: 0, subjectDigest: [256] }), false);
  assert.equal(isViewAction({ type: "switchModel", available: [""] }), false);
  assert.equal(isViewAction({ type: "switchEffort", model: "m".repeat(201) }), false);
  assert.equal(isViewAction({ type: "switchEffort" }), false);
  assert.equal(
    isViewAction({ type: "switchMode", available: Array.from({ length: 65 }, () => "m") }),
    false,
  );
  assert.equal(isViewAction({ type: "removeAttachment", index: -1 }), false);
  assert.equal(isViewAction({ type: "removeAttachment", index: 8 }), false, "bounded by the image limit");
  assert.equal(isViewAction({ type: "removeAttachment", index: "0" }), false);
  assert.equal(isViewAction(null), false);
  assert.equal(isViewAction([]), false);
  assert.equal(isViewAction("interrupt"), false);
});
