import assert from "node:assert/strict";
import test from "node:test";

import { markFrame, paintMark } from "./openingMark";

test("the mark is four arms and a quarter turn moves each one to the next corner", () => {
  const frames = Array.from({ length: 4 }, (_, at) => markFrame(at));
  const corners = frames.map((frame) => [
    frame[0]?.[0],
    frame[0]?.[4],
    frame[2]?.[4],
    frame[2]?.[0],
  ]);
  const arms = ["╭", "╮", "╯", "╰"];
  for (const frame of corners) {
    // Every frame is the same four arms, so the mark never gains or loses one while it turns.
    assert.deepEqual([...frame].sort(), [...arms].sort());
  }
  // A quarter turn: the arm at the top left moves to the top right, and so on around.
  for (let at = 0; at < 3; at += 1) {
    const before = corners[at];
    const after = corners[at + 1];
    assert.ok(before && after);
    assert.deepEqual([after[1], after[2], after[3], after[0]], before);
  }
  assert.deepEqual(markFrame(4), frames[0], "the fifth frame is the first again");
});

test("a frame is painted in the middle of the pane, whatever its size", () => {
  const wide = paintMark(0, 120, 40);
  // Clear first: the pane may have been resized, and half of an old block left behind reads as a defect.
  assert.ok(wide.startsWith("\x1b[2J"));
  // A 120x40 pane puts the three-row block at row 18 and its five columns at 57, which is its middle.
  assert.ok(wide.includes("\x1b[18;57H"), wide.slice(0, 60));
  assert.ok(wide.includes("\x1b[20;57H"), "the block's last line");
  // A pane too small for the block still gets a frame rather than an escape with a zero or negative position.
  const tiny = paintMark(0, 2, 1);
  assert.ok(tiny.includes("\x1b[1;1H"));
  assert.ok(!tiny.includes("[0;"));
  assert.ok(!tiny.includes("-"));
});
