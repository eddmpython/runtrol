import assert from "node:assert/strict";
import test from "node:test";

import { NO_ACTIVITY, activityAfter } from "./sessionActivity";

const frame = (body: Record<string, unknown>): unknown => ({ session: "s", body });

test("a running tool names the row, in the provider's own word, and its end clears it", () => {
  // Claude Code names its tool and classifies nothing; the row shows the name alone, as the page does.
  const running = activityAfter(NO_ACTIVITY, frame({
    event: "toolCall",
    status: "inProgress",
    payload: { name: "Bash" },
  }));
  assert.equal(running.tool, "Bash");
  // The result frame names no tool; the word stays.
  const stillRunning = activityAfter(running, frame({ event: "toolCallUpdate", status: "inProgress", payload: {} }));
  assert.equal(stillRunning.tool, "Bash");
  const done = activityAfter(stillRunning, frame({ event: "toolCallUpdate", status: "completed", payload: {} }));
  assert.equal(done.tool, null);
});

test("a classified tool reads as its verb and target, and the turn's end clears whatever ran", () => {
  const running = activityAfter(NO_ACTIVITY, frame({
    event: "toolCall",
    kind: "execute",
    status: "inProgress",
    payload: { title: "npm test" },
  }));
  assert.equal(running.tool, "Run npm test");
  const ended = activityAfter(running, frame({ event: "turn", step: "ended" }));
  assert.equal(ended.tool, null);
});

test("a sign-in notice raises the flag and a fresh attachment lowers it", () => {
  const needs = activityAfter(NO_ACTIVITY, frame({ event: "notice", code: "authRequired", level: "error" }));
  assert.equal(needs.signInNeeded, true);
  const attached = activityAfter(needs, frame({ event: "attached", native: "x" }));
  assert.equal(attached.signInNeeded, false);
});

test("events that say nothing about activity return the same object", () => {
  const same = activityAfter(NO_ACTIVITY, frame({ event: "agentMessageChunk", content: {} }));
  assert.equal(same, NO_ACTIVITY);
  assert.equal(activityAfter(NO_ACTIVITY, "not a frame"), NO_ACTIVITY);
});
