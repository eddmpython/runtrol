import assert from "node:assert/strict";
import test from "node:test";

import { planEntriesOf, planGlyph } from "./plan";

test("reads the ACP plan shape in the service's own order", () => {
  const entries = planEntriesOf({
    payload: {
      sessionUpdate: "plan",
      entries: [
        { content: "Read the failing test", priority: "high", status: "completed" },
        { content: "Fix the parser", status: "in_progress" },
        { content: "Run the suite", status: "pending" },
      ],
    },
  });
  assert.deepEqual(entries, [
    { content: "Read the failing test", status: "completed" },
    { content: "Fix the parser", status: "in_progress" },
    { content: "Run the suite", status: "pending" },
  ]);
});

test("an unknown status reads as pending, never as done", () => {
  const entries = planEntriesOf({ payload: { entries: [{ content: "step", status: "somethingNew" }] } });
  assert.deepEqual(entries, [{ content: "step", status: "pending" }]);
  assert.equal(planGlyph("pending"), "○");
  assert.equal(planGlyph("in_progress"), "◐");
  assert.equal(planGlyph("completed"), "●");
});

test("a payload without readable entries yields nothing so the caller can fall back", () => {
  assert.deepEqual(planEntriesOf({}), []);
  assert.deepEqual(planEntriesOf({ payload: { entries: "not a list" } }), []);
  assert.deepEqual(planEntriesOf({ payload: { entries: [{ status: "pending" }, { content: "   " }] } }), []);
});

test("entry count and content length are bounded", () => {
  const entries = planEntriesOf({
    payload: {
      entries: Array.from({ length: 100 }, (unused, index) => ({
        content: index === 0 ? "x".repeat(1000) : `step ${index}`,
        status: "pending",
      })),
    },
  });
  assert.equal(entries.length, 64);
  assert.equal(entries[0]?.content.length, 512);
});
