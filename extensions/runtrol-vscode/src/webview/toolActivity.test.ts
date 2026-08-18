import assert from "node:assert/strict";
import test from "node:test";

import { toolActivityLine, toolActivityLineKeeping, toolActivityOf } from "./toolActivity";

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
  // The classification stays neutral, and the line drops the neutral word rather than printing it in front of the
  // only real information the service gave.
  assert.equal(toolActivityLine(unclassified), "something...");
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

test("a service that only named its tool shows that name, not a filler verb", () => {
  // Claude Code reports a tool's name and its raw arguments, never a display label the way the Agent Client
  // Protocol does. Reading those arguments to build one would be interpreting what a provider never offered for
  // display, so the name is all there is. "Tool Read" puts a filler word in front of the only real information on
  // the line, and the reader is left parsing our vocabulary instead of reading theirs.
  assert.equal(
    toolActivityLine(toolActivityOf({ status: "completed", payload: { title: "Read" } })),
    "Read",
  );
  assert.equal(
    toolActivityLine(toolActivityOf({ status: "inProgress", payload: { title: "Bash" } })),
    "Bash...",
  );
  // A service that classified its tool still reads as verb plus target.
  assert.equal(
    toolActivityLine(toolActivityOf({ kind: "read", status: "completed", payload: { title: "src/main.rs" } })),
    "Read src/main.rs",
  );
  // And one that gave neither still says something rather than nothing.
  assert.equal(toolActivityLine(toolActivityOf({ status: "completed" })), "Tool");
});

test("a tool's own name is used when the service gave no display label", () => {
  // Claude Code reports `name` and raw arguments; the Agent Client Protocol reports a `title` meant for a reader.
  // Both are names the service put in its own frame, so both are safe to show. A service that gives both keeps its
  // label, because that is the one it wrote for a person.
  assert.equal(
    toolActivityOf({ status: "completed", payload: { name: "Read" } }).target,
    "Read",
  );
  assert.equal(
    toolActivityOf({ status: "completed", payload: { title: "Read src/main.rs", name: "Read" } }).target,
    "Read src/main.rs",
  );
});

test("nothing but those two names is read, whatever else the payload carries", () => {
  // The rule this module exists for. Raw input, output and diffs are the conversation, and composing a label out of
  // them would interpret what no service offered for display.
  const activity = toolActivityOf({
    kind: "read",
    status: "completed",
    payload: {
      title: "Read src/main.rs",
      name: "Read",
      input: { file_path: "SECRET_PATH" },
      output: "SECRET_OUTPUT",
      content: [{ type: "diff", oldText: "SECRET_OLD" }],
      locations: ["SECRET_LOCATION"],
    },
  });
  const rendered = JSON.stringify(activity);
  for (const secret of ["SECRET_PATH", "SECRET_OUTPUT", "SECRET_OLD", "SECRET_LOCATION"]) {
    assert.equal(rendered.includes(secret), false, `${secret} reached the activity line`);
  }
});

test("a call keeps its name when the result arrives", () => {
  // Claude Code sends the name on `tool_use` and only an identifier on `tool_result`. Redrawing the row from the
  // result alone renamed every finished call to "Tool", which is the moment the reader most wants to know what ran.
  const call = toolActivityOf({ status: "inProgress", payload: { name: "Bash" } });
  assert.equal(toolActivityLineKeeping(call, ""), "Bash...");

  const result = toolActivityOf({ status: "completed", payload: { tool_use_id: "toolu_01" } });
  assert.equal(result.target, "", "the result frame carries no name of its own");
  assert.equal(toolActivityLineKeeping(result, "Bash"), "Bash");
});

test("a frame that carries its own name is not overridden by the remembered one", () => {
  // The Agent Client Protocol repeats the label on every update. Preferring the stale one would freeze a title
  // the service is still refining.
  const update = toolActivityOf({ kind: "edit", status: "completed", payload: { title: "src/main.rs" } });
  assert.equal(toolActivityLineKeeping(update, "something older"), "Edit src/main.rs");
});

test("a failed call is still named", () => {
  const failure = toolActivityOf({ status: "failed", payload: { tool_use_id: "toolu_01" } });
  assert.equal(toolActivityLineKeeping(failure, "Bash"), "Bash · failed");
  assert.equal(toolActivityLine(failure), "Tool · failed", "with nothing remembered there is nothing to name");
});
