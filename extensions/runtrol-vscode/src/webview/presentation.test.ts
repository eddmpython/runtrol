import assert from "node:assert/strict";
import test from "node:test";

import { coalesceChunks, record, textOf } from "./presentation";

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
