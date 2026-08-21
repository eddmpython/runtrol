import assert from "node:assert/strict";
import test from "node:test";

import { MAX_DIFF_CHARACTERS, declaredDiffs, isDeclaredDiff } from "./toolDiff";

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

test("a codex file change declared inside the item is a diff too", () => {
  const finding = declaredDiffs({
    payload: {
      item: { type: "fileChange", id: "exec-1", changes: [{ path: "hello.txt", diff: "+hi" }] },
    },
  });
  assert.deepEqual(finding.diffs, [{ kind: "unified", path: "hello.txt", text: "+hi" }]);
  assert.deepEqual([...finding.consumed], ["item"]);
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
        diff: index === 0 ? "+".repeat(MAX_DIFF_CHARACTERS + 1000) : "+x",
      })),
    },
  });
  assert.equal(finding.diffs.length, 8);
  const first = finding.diffs[0];
  assert.ok(
    first?.kind === "unified" && first.text.length === MAX_DIFF_CHARACTERS + 4,
    "bounded with an ellipsis line",
  );
  assert.equal(finding.consumed.has("changes"), false, "truncated coverage is not full coverage");
});

test("only a bounded declared change crosses back from the page to the editor", () => {
  assert.ok(isDeclaredDiff({ kind: "oldNew", path: "a.rs", oldText: "a", newText: "b" }));
  assert.ok(isDeclaredDiff({ kind: "unified", path: "", text: "+x" }));
  assert.equal(isDeclaredDiff({ kind: "unified", path: "a", text: 3 }), false);
  assert.equal(isDeclaredDiff({ kind: "other", path: "a", text: "x" }), false);
  assert.equal(isDeclaredDiff({ kind: "unified", path: "a", text: "x".repeat(MAX_DIFF_CHARACTERS + 5) }), false);
  assert.equal(isDeclaredDiff("nope"), false);
});
