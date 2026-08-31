import assert from "node:assert/strict";
import test from "node:test";

import { hasVisibleText, markPixels, paintMark } from "./openingMark";

const CORAL = "38;2;245;101;101";
const CORAL_LIT = "38;2;250;178;178";

test("the mark is the brand's raster: two coral arms and two ink arms around an empty centre", () => {
  const pixels = markPixels();
  assert.equal(pixels.length, 16);
  assert.ok(pixels.every((row) => row.length === 16), "the grid is square");
  const at = (x: number, y: number) => pixels[y]?.[x] ?? null;
  // The vertical bars leave the top edge two wide: coral in columns 5..6, ink in 9..10.
  assert.deepEqual([at(4, 0), at(5, 0), at(6, 0), at(7, 0)], [null, "accent", "accent", null]);
  assert.deepEqual([at(8, 0), at(9, 0), at(10, 0), at(11, 0)], [null, "ink", "ink", null]);
  // The centre gap is two wide in both directions, so the arms never touch.
  for (let gap = 7; gap <= 8; gap += 1) {
    for (let along = 0; along < 16; along += 1) {
      assert.equal(at(along, gap), null, `row ${gap} column ${along} is empty`);
      assert.equal(at(gap, along), null, `column ${gap} row ${along} is empty`);
    }
  }
  // A quarter turn carries each arm onto the next one and swaps the colours: the mark's rotational symmetry,
  // which is what makes it ours rather than four unrelated strokes.
  const swapped = (pixel: ReturnType<typeof at>) => (pixel === "accent" ? "ink" : pixel === "ink" ? "accent" : null);
  for (let y = 0; y < 16; y += 1) {
    for (let x = 0; x < 16; x += 1) {
      assert.equal(at(15 - y, x), swapped(at(x, y)), `(${x}, ${y}) turned a quarter`);
    }
  }
});

test("the block is half-block cells in the two colours, and its ink is the terminal's own foreground", () => {
  // Importing the module already proved every cell holds one colour: it throws on a cell that straddles
  // coral and ink. What is left to see is that the frame really carries both colours and all three glyphs.
  const frame = paintMark(0, 120, 40);
  assert.ok(frame.includes(`;${CORAL}m`), "the accent arms are coral");
  assert.ok(frame.includes(";39m"), "the ink arms take the default foreground");
  for (const glyph of ["▀", "▄", "█"]) assert.ok(frame.includes(glyph), `the block uses ${glyph}`);
});

test("a coral light sweeps the mark left to right, rests, and repeats", () => {
  // Block row 2 holds pixel rows 4 and 5, where the horizontal bars run out to both edges.
  const row = (at: number) => paintedRow(paintMark(at, 120, 40), 17 + 2, 53);
  const litColumns = (at: number) => row(at).flatMap((cell, x) => (
    cell.glyph !== " " && cell.style.includes(CORAL_LIT) ? [x] : []
  ));
  const touched = (at: number) => row(at).some((cell) => cell.glyph !== " " && (
    cell.style.includes("38;2;250;178;178") || cell.style.includes("38;2;248;140;140")
  ));
  // The light starts outside the block and the mark stands unlit for a few frames before it enters.
  assert.deepEqual([0, 1, 2, 3].map(touched), [false, false, false, false], "the rest before a pass");
  assert.deepEqual(litColumns(4), [0], "the core reaches the first column");
  assert.deepEqual(litColumns(6), [2, 3, 4]);
  assert.deepEqual(litColumns(7), [4, 5, 6], "and moves to the right by two cells a frame");
  assert.equal(touched(13), true, "the edge is still on the last column as the light leaves");
  assert.deepEqual([14, 15, 16].map(touched), [false, false, false], "the rest after a pass");
  assert.deepEqual(litColumns(6 + 17), litColumns(6), "the pass repeats");
  // The light has a softer edge one cell wide, and beyond it the arms stand in their own colours: nothing dimmed.
  const cells = row(6);
  assert.equal(cells[5]?.style, "0;38;2;248;140;140", "the edge of the light");
  assert.equal(cells[0]?.style, `0;${CORAL}`, "the rest of the arm stays coral at full strength");
  assert.equal(cells[10]?.style, "0;39", "the ink arm is the default foreground at full strength");
  assert.equal(cells[7]?.glyph, " ", "the centre gap is a plain space");
});

test("a frame is painted in the middle of the pane, whatever its size", () => {
  const wide = paintMark(0, 120, 40);
  // Clear first: the pane may have been resized, and half of an old block left behind reads as a defect.
  assert.ok(wide.startsWith("\x1b[2J"));
  // A 120x40 pane puts the eight-row block on rows 17..24 and its 16 columns at 53..68.
  assert.ok(wide.includes("\x1b[17;53H"), wide.slice(0, 60));
  assert.ok(wide.includes("\x1b[24;53H"), "the block's last row");
  assert.equal(wide.match(/\x1b\[\d+;\d+H/gu)?.length, 8, "eight rows painted");
  assert.equal(paintedRow(wide, 17, 53).length, 16, "sixteen cells each");
  // A pane too small for the block gets the part that fits, from the block's top left, and never a wrap: an
  // escape with a zero or negative position, or a row longer than the pane, would scroll or overlap.
  const tiny = paintMark(0, 2, 1);
  assert.ok(tiny.startsWith("\x1b[2J\x1b[1;1H"));
  assert.equal(tiny.match(/\x1b\[\d+;\d+H/gu)?.length, 1, "one row fits");
  assert.equal(paintedRow(tiny, 1, 1).length, 2, "two cells fit");
  const narrow = paintMark(0, 10, 4);
  assert.equal(narrow.match(/\x1b\[\d+;\d+H/gu)?.length, 4);
  for (let screenRow = 1; screenRow <= 4; screenRow += 1) assert.equal(paintedRow(narrow, screenRow, 1).length, 10);
  assert.equal(paintMark(0, 0, 0), "\x1b[2J", "a pane with no cells is only cleared");
  assert.ok(!/\x1b\[0;\d+H/u.test(narrow), "no row zero");
  assert.ok(!narrow.includes("-"), "no negative position");
});

test("only a printable character counts as visible; escape sequences and blanks do not", () => {
  // What a service sends before it draws: clears, cursor moves, colour resets, cursor hiding, a window
  // title, charset selection, and whitespace. None of it is something a person can see.
  for (const silent of [
    "", " \t\r\n", "\x1b[2J", "\x1b[H", "\x1b[?25l", "\x1b[0m", "\x1b[1;1H", "\x1b7", "\x1b(B",
    "\x1b]0;title with words\x07", "\x1b]0;title with words\x1b\\", "\x1b[2J\x1b[H\x1b[?25l\x1b[0m",
  ]) {
    assert.equal(hasVisibleText(silent), false, JSON.stringify(silent));
  }
  for (const visible of ["a", "\x1b[2J\x1b[Hx", "\x1b[31m>\x1b[0m", "\x1b]0;t\x07hi", "\x1b7~"]) {
    assert.equal(hasVisibleText(visible), true, JSON.stringify(visible));
  }
});

interface PaintedCell {
  readonly glyph: string;
  /// The SGR parameters in force when the glyph was drawn, as they stand between `ESC [` and `m`.
  readonly style: string;
}

/// The cells of one painted block row, starting at the cursor move to `screenRow`, `left`.
function paintedRow(frame: string, screenRow: number, left: number): PaintedCell[] {
  const move = `\x1b[${screenRow};${left}H`;
  const start = frame.indexOf(move);
  assert.notEqual(start, -1, `the frame paints row ${screenRow}`);
  let cursor = start + move.length;
  let style = "";
  const cells: PaintedCell[] = [];
  while (cursor < frame.length) {
    if (frame[cursor] !== "\x1b") {
      cells.push({ glyph: frame[cursor] ?? "", style });
      cursor += 1;
      continue;
    }
    const styleEnd = frame.indexOf("m", cursor);
    const moveEnd = frame.indexOf("H", cursor);
    // A cursor move is the next row; the row itself only carries SGR sequences.
    if (styleEnd === -1 || (moveEnd !== -1 && moveEnd < styleEnd)) break;
    style = frame.slice(cursor + 2, styleEnd);
    cursor = styleEnd + 1;
  }
  return cells;
}
