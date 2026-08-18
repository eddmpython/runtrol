import assert from "node:assert/strict";
import test from "node:test";

import { toolDetail } from "./toolDetail";

test("what a tool printed is shown as the lines it printed", () => {
  // Seen on screen: JSON.stringify wrote the newline as backslash-n, so a command's output became one line
  // running off the right edge of a panel two hundred pixels wide. The point of opening the panel is to read it.
  const result = toolDetail({
    payload: {
      type: "tool_result",
      tool_use_id: "toolu_01",
      content: "On branch main\nnothing to commit",
    },
  });
  assert.equal(
    result,
    ["type: tool_result", "tool_use_id: toolu_01", "content:", "  On branch main", "  nothing to commit"].join("\n"),
  );
  assert.ok(!result.includes("\\n"), "no escape sequence survives onto the screen");
});

test("a nested shape keeps its nesting", () => {
  // Claude Code puts the arguments under `input`, and flattening them would lose which tool they belong to.
  assert.equal(
    toolDetail({
      payload: {
        type: "tool_use",
        id: "toolu_01",
        input: { command: "git status", description: "Show working tree status" },
      },
    }),
    [
      "type: tool_use",
      "id: toolu_01",
      "input:",
      "  command: git status",
      "  description: Show working tree status",
    ].join("\n"),
  );
});

test("the names already on the summary line are not repeated inside the panel", () => {
  assert.equal(toolDetail({ payload: { name: "Bash", title: "git status" } }), "");
  assert.equal(toolDetail({ payload: { name: "Bash", input: { command: "ls" } } }), "input:\n  command: ls");
});

test("every key the service sent survives, in the order it sent them", () => {
  // The panel is a transport, not an editor. Reordering or dropping a field is rewriting what arrived.
  const detail = toolDetail({ payload: { zebra: 1, alpha: 2, middle: 3 } });
  assert.equal(detail, "zebra: 1\nalpha: 2\nmiddle: 3");
});

test("a list is shown by position rather than run together", () => {
  assert.equal(
    toolDetail({ payload: { locations: [{ path: "a.rs" }, { path: "b.rs" }] } }),
    ["locations:", "  0:", "    path: a.rs", "  1:", "    path: b.rs"].join("\n"),
  );
});

test("nothing at all is nothing, not an empty panel", () => {
  // A row with an empty panel is a disclosure arrow that opens onto blank space.
  assert.equal(toolDetail({}), "");
  assert.equal(toolDetail({ payload: {} }), "");
  assert.equal(toolDetail({ payload: { touched: [] } }), "touched: (none)");
});

test("a result too large to show is cut and says so", () => {
  // A tool result can be a whole file, and a panel that grows without limit makes the transcript unscrollable.
  const detail = toolDetail({ payload: { content: "x".repeat(9_000) } });
  assert.ok(detail.length < 4_100, "the panel stays bounded");
  assert.ok(detail.endsWith("\n..."), "and the reader is told it was cut");
});

test("a value that is not text still reaches the screen", () => {
  assert.equal(
    toolDetail({ payload: { ok: true, count: 3, missing: null } }),
    "ok: true\ncount: 3\nmissing: null",
  );
});
