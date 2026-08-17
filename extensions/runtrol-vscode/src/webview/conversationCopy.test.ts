import assert from "node:assert/strict";
import test from "node:test";

import { conversationEmptyCopy, sendShortcutHint } from "./conversationCopy";

test("an empty editor points at the sidebar instead of a generic start screen", () => {
  assert.deepEqual(conversationEmptyCopy(null, "Claude Code", null), {
    heading: "Start a conversation",
    detail: "Every conversation on this machine is listed in the sidebar.",
    tone: "hero",
  });
});

test("an open chat keeps a quiet named empty state until the first message", () => {
  assert.deepEqual(conversationEmptyCopy({ lifecycle: "hotIdle" }, "Claude Code", "sideProject"), {
    heading: "sideProject",
    detail: "Message Claude Code below.",
    tone: "quiet",
  });
  assert.equal(
    conversationEmptyCopy({ lifecycle: "hotRunning" }, "Codex CLI", "sideProject").heading,
    "Codex CLI is working",
  );
  assert.equal(
    conversationEmptyCopy({ lifecycle: "cold" }, "Claude Code", "sideProject").detail,
    "Reopening the saved conversation.",
  );
});

test("the composer hint teaches the newline, because Enter already sends", () => {
  assert.equal(sendShortcutHint(), "Shift+Enter for a new line");
});
