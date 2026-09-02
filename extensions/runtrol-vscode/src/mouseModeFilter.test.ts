import assert from "node:assert/strict";
import test from "node:test";

import { MouseModeFilter } from "./mouseModeFilter";

test("the CLI's mouse switches leave the stream and everything else stays", () => {
  const filter = new MouseModeFilter();
  assert.equal(filter.filter("a\x1b[?1000h\x1b[?1006hb"), "ab");
  assert.equal(filter.filter("\x1b[?1002l\x1b[?1003l"), "");
  // Other private modes in the same sequence stay; only the mouse parameters leave.
  assert.equal(filter.filter("\x1b[?1049;1000;25h"), "\x1b[?1049;25h");
  assert.equal(filter.filter("\x1b[?25l\x1b[2J\x1b[H"), "\x1b[?25l\x1b[2J\x1b[H");
  assert.equal(filter.filter("\x1b[?1h"), "\x1b[?1h");
  assert.equal(filter.filter("\x1b[6n\x1b[c\x1b]0;title\x07한글"), "\x1b[6n\x1b[c\x1b]0;title\x07한글");
});

test("a mouse switch split across two writes is still taken out", () => {
  const filter = new MouseModeFilter();
  assert.equal(filter.filter("x\x1b[?10") + filter.filter("00hy"), "xy");
  const splitAtBracket = new MouseModeFilter();
  assert.equal(splitAtBracket.filter("\x1b") + splitAtBracket.filter("[?1006h!"), "!");
});

test("a reset forgets the unfinished tail so a restarted stream never inherits it", () => {
  const filter = new MouseModeFilter();
  assert.equal(filter.filter("\x1b[?10"), "");
  filter.reset();
  assert.equal(filter.filter("00h"), "00h");
});

test("a stray ESC longer than the carry passes through rather than growing the carry", () => {
  const filter = new MouseModeFilter();
  const long = "\x1b[" + "9".repeat(40);
  assert.equal(filter.filter(long), long);
});
