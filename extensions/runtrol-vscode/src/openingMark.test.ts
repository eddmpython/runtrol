import assert from "node:assert/strict";
import test from "node:test";

import { markFrame, paintMark } from "./openingMark";

test("the mark is the four arms in their corners and stands still", () => {
  // The symbol does not spin: it is the same four arms every frame, and the light is what moves.
  const shape = markFrame();
  assert.deepEqual([shape[0]?.[0], shape[0]?.[4], shape[2]?.[4], shape[2]?.[0]], ["╭", "╮", "╯", "╰"]);
  assert.equal(shape[1], "     ", "the middle row is empty between the arms");
});

test("a coral light sweeps the symbol left to right and repeats", () => {
  // The lit cell is the one carrying the coral escape. As frames advance it moves right across the columns,
  // and after a full period it is back where it began: a repeating left-to-right pass.
  const coral = "\x1b[1;38;2;245;101;101m";
  const litColumn = (at: number): number | null => {
    const frame = paintMark(at, 40, 10);
    // The top row's first arm is at the mark's left column; find which drawn column carries the coral escape.
    for (let column = 0; column < 5; column += 1) {
      // Each cell is "<style><char>"; the lit one is prefixed by the coral escape.
      const cellStart = frame.indexOf(coral);
      if (cellStart !== -1 && column === leftmostLitColumn(frame, coral)) return column;
    }
    return leftmostLitColumn(frame, coral);
  };
  const early = litColumn(2);
  const later = litColumn(4);
  assert.ok(early !== null && later !== null, "the light is on the symbol");
  assert.ok((later as number) > (early as number), "and it moved to the right");
  // The period brings it back: same lit column at `at` and `at + period`.
  const period = 5 + 4;
  assert.equal(litColumn(3), litColumn(3 + period), "the pass repeats");
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

/// The column of the first arm that carries the coral escape in a painted top row, or null.
function leftmostLitColumn(frame: string, coral: string): number | null {
  // The top row starts after the first cursor move. Walk its five cells, each "<style><char>".
  const arms = "╭   ╮";
  let column: number | null = null;
  let cursor = frame.indexOf("H", frame.indexOf("\x1b[2J")) + 1;
  for (let x = 0; x < arms.length; x += 1) {
    const litHere = frame.startsWith(coral, cursor);
    const style = litHere ? coral : frame.startsWith("\x1b[2m", cursor) ? "\x1b[2m" : "\x1b[0m";
    if (litHere && column === null) column = x;
    cursor += style.length + 1;
  }
  return column;
}
