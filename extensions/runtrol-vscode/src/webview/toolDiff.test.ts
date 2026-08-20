import assert from "node:assert/strict";
import test from "node:test";

import { declaredDiffs, unifiedLineKind } from "./toolDiff";

test("reads the two measured shapes and nothing else", () => {
  const finding = declaredDiffs({
    payload: {
      content: [{ type: "diff", path: "src/lib.rs", oldText: "a", newText: "b" }],
      changes: [{ path: "src/main.rs", diff: "+safe\n-gone" }],
    },
  });
  assert.deepEqual(finding.diffs, [
    { kind: "oldNew", path: "src/lib.rs", oldText: "a", newText: "b" },
    { kind: "unified", path: "src/main.rs", text: "+safe\n-gone" },
  ]);
  assert.deepEqual([...finding.consumed].sort(), ["changes", "content"]);
});

test("a tool argument that merely looks like a patch is not a diff", () => {
  // Claude Code's edit input carries old_string/new_string as arguments; the service never declared
  // them as a change, so this product must not render them as one.
  const finding = declaredDiffs({
    payload: {
      input: { old_string: "a", new_string: "b" },
      content: [{ type: "content", text: "hello" }],
    },
  });
  assert.deepEqual(finding.diffs, []);
  assert.equal(finding.consumed.size, 0);
});

test("a mixed content list renders its diffs but is never marked consumed", () => {
  const finding = declaredDiffs({
    payload: {
      content: [
        { type: "diff", newText: "fresh" },
        { type: "content", text: "and a sentence" },
      ],
    },
  });
  assert.equal(finding.diffs.length, 1);
  assert.equal(finding.consumed.has("content"), false);
});

test("diff text and count are bounded", () => {
  const finding = declaredDiffs({
    payload: {
      changes: Array.from({ length: 12 }, (unused, index) => ({
        path: `file-${index}`,
        diff: index === 0 ? "+".repeat(5000) : "+x",
      })),
    },
  });
  assert.equal(finding.diffs.length, 8);
  const first = finding.diffs[0];
  assert.ok(first?.kind === "unified" && first.text.length === 4004, "bounded with an ellipsis line");
  assert.equal(finding.consumed.has("changes"), false, "truncated coverage is not full coverage");
});

test("unified lines are coloured only by their own first characters", () => {
  assert.equal(unifiedLineKind("+added"), "add");
  assert.equal(unifiedLineKind("-removed"), "del");
  assert.equal(unifiedLineKind("@@ -1 +1 @@"), "hunk");
  assert.equal(unifiedLineKind(" context"), "context");
});
