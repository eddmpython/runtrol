import assert from "node:assert/strict";
import test from "node:test";

import { toolActivityLine, toolActivityOf } from "./toolActivity";

test("the provider's own classification becomes a word a person reads", () => {
  const edit = toolActivityOf({ kind: "edit", status: "inProgress", payload: { title: "src/main.rs" } });
  assert.deepEqual(edit, { verb: "Edit", target: "src/main.rs", state: "running" });
  assert.equal(toolActivityLine(edit), "Edit src/main.rs...");

  const run = toolActivityOf({ kind: "execute", status: "completed", payload: { title: "cargo test" } });
  assert.equal(toolActivityLine(run), "Run cargo test");
});

test("a provider that does not classify its tools gets the neutral word, not a guess", () => {
  // Runtrol never infers a kind from a tool name. A name-to-kind table goes stale the first time a vendor
  // renames a tool, and a wrong verb is worse than a plain one.
  const unclassified = toolActivityOf({ status: "inProgress", payload: { title: "something" } });
  assert.equal(unclassified.verb, "Tool");
  assert.equal(toolActivityLine(unclassified), "Tool something...");
});

test("a failure says so, because it is the one a reader has to notice", () => {
  const failed = toolActivityOf({ kind: "execute", status: "failed", payload: { title: "cargo test" } });
  assert.equal(failed.state, "failed");
  assert.equal(toolActivityLine(failed), "Run cargo test · failed");
});

test("no title means no invented target", () => {
  const bare = toolActivityOf({ kind: "read", status: "completed" });
  assert.equal(bare.target, "");
  assert.equal(toolActivityLine(bare), "Read");
});

test("nothing but the label is read out of the payload", () => {
  // Raw input, raw output, diffs and terminal bytes are the conversation. This surface transports them and does
  // not interpret them, so a payload full of them still yields only the label.
  const activity = toolActivityOf({
    kind: "edit",
    status: "completed",
    payload: {
      title: "src/lib.rs",
      rawInput: { path: "src/lib.rs", contents: "SECRET" },
      rawOutput: "diff --git a/src/lib.rs",
      content: [{ type: "diff", oldText: "a", newText: "b" }],
      locations: [{ path: "src/lib.rs", line: 12 }],
    },
  });

  assert.deepEqual(activity, { verb: "Edit", target: "src/lib.rs", state: "done" });
  assert.equal(JSON.stringify(activity).includes("SECRET"), false);
});

test("an unrecognised status is unknown rather than a claim", () => {
  const odd = toolActivityOf({ kind: "fetch", status: "somethingNew", payload: { title: "https://x" } });
  assert.equal(odd.state, "unknown");
  assert.equal(toolActivityLine(odd), "Fetch https://x");
});
