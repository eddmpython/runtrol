import assert from "node:assert/strict";
import test from "node:test";

import { markFrame, paintMark } from "./openingMark";

test("the mark stands still and one dot goes around it", () => {
  const frames = Array.from({ length: 8 }, (_, at) => markFrame(at));
  for (const frame of frames) {
    assert.equal(frame.length, 3);
    // The mark itself never moves: it is the thing the dot travels around.
    assert.equal(frame[1]?.[2], "✦");
    assert.equal(frame.join("").split("•").length - 1, 1, "exactly one position is lit");
  }
  // Eight frames, eight different places, and the ninth is the first again.
  const lit = frames.map((frame) => frame.join("").indexOf("•"));
  assert.equal(new Set(lit).size, 8);
  assert.deepEqual(markFrame(8), frames[0]);
});

test("a frame is painted in the middle of the pane, whatever its size", () => {
  const wide = paintMark(0, 120, 40);
  // Clear first: the pane may have been resized, and half of an old block left behind reads as a defect.
  assert.ok(wide.startsWith("\x1b[2J"));
  // A 120x40 pane puts the three-row block at row 18 and its five columns at 57, which is its middle.
  assert.ok(wide.includes("[18;57H"), wide.slice(0, 60));
  assert.ok(wide.includes("[19;57H"), "the mark's own line");
  // A pane too small for the block still gets a frame rather than an escape with a zero or negative position.
  const tiny = paintMark(0, 2, 1);
  assert.ok(tiny.includes("\x1b[1;1H"));
  assert.ok(!tiny.includes("[0;"));
  assert.ok(!tiny.includes("-"));
});
