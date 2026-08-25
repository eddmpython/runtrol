import assert from "node:assert/strict";
import test from "node:test";

import { coalesceChunks, presentationOf, record, textOf } from "./presentation";

function chunk(message: string, text: string, delta = true, event = "agentMessageChunk"): unknown {
  return { body: { event, message_id: message, delta, content: { text } } };
}

test("adjacent deltas for one visible message are combined without changing text", () => {
  const result = coalesceChunks([chunk("one", "alpha"), chunk("one", " beta"), chunk("one", " gamma")]);
  assert.equal(result.length, 1);
  assert.equal(textOf(record(record(result[0])?.body)?.content), "alpha beta gamma");
});

test("message, side, order, and non-delta boundaries prevent combination", () => {
  const frames = [
    chunk("one", "a"),
    chunk("two", "b"),
    chunk("two", "c", true, "agentThoughtChunk"),
    chunk("two", "d", false),
    { body: { event: "turn", step: "finished" } },
  ];
  assert.deepEqual(coalesceChunks(frames), frames);
});

test("provider-owned content shapes keep their exact visible text", () => {
  assert.equal(textOf("plain"), "plain");
  assert.equal(textOf({ delta: "delta" }), "delta");
  assert.equal(textOf({ item: { text: "item" } }), "item");
  assert.equal(textOf({ content: [{ text: "one" }, { text: "two" }] }), "onetwo");
});

test("shared presentation data drives message, status, approval, and discarded events", () => {
  assert.deepEqual(presentationOf("agentMessageChunk"), {
    kind: "message",
    side: "theirs",
    labelKey: "message.agent",
  });
  assert.equal(presentationOf("attached")?.kind, "status");
  assert.equal(presentationOf("approvalRequested")?.kind, "approval");
  assert.equal(presentationOf("unmapped")?.kind, "discard");
  assert.equal(presentationOf("futureProviderEvent"), null);
});

test("a streaming delta is text, not an object that reads as nothing", () => {
  // Claude Code streams in Anthropic's content-block shape. Reading `delta` as a string returned "" for every
  // chunk, so finished messages appeared and the typing did not: a reply landed in one lump after the agent had
  // already stopped. This is what "the conversation as that CLI shows it" fails on first.
  assert.equal(textOf({ type: "content_block_delta", delta: { type: "text_delta", text: "ok" } }), "ok");
  assert.equal(textOf({ type: "content_block_delta", delta: { type: "thinking_delta", thinking: "hm" } }), "hm");
  // The older flat shapes still work.
  assert.equal(textOf({ delta: "ok" }), "ok");
  assert.equal(textOf({ text: "ok" }), "ok");
});

test("a thought inside a content array is shown, not skipped", () => {
  assert.equal(
    textOf({ message: { content: [{ type: "thinking", thinking: "weighing it" }] } }),
    "weighing it",
  );
  assert.equal(
    textOf({ content: [{ type: "text", text: "a" }, { type: "thinking", thinking: "b" }] }),
    "ab",
  );
});
