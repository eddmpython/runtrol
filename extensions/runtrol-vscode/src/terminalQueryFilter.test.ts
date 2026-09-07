import assert from "node:assert/strict";
import test from "node:test";

import { OWNED_QUERY_LITERALS, QUERY_CARRY_LIMIT, QUERY_MODE_DIGITS, QUERY_TERMINATOR, TerminalQueryFilter } from "./terminalQueryFilter";
import { MouseModeFilter } from "./mouseModeFilter";
import agreement from "../../../crates/runtrol-terminal-protocol/fixtures/queries.json";

test("Studio and the Rust host agree on the complete owned query grammar and bounds", () => {
  assert.deepEqual(OWNED_QUERY_LITERALS, agreement.literalBodies);
  assert.equal(QUERY_CARRY_LIMIT, agreement.carryLimit);
  assert.equal(QUERY_MODE_DIGITS, agreement.modeDigits);
  assert.equal(QUERY_TERMINATOR, agreement.replacement);
  for (const query of agreement.queries) {
    for (let split = 0; split <= query.length; split += 1) {
      const filter = new TerminalQueryFilter();
      assert.equal(filter.filter("before" + query.slice(0, split)) + filter.filter(query.slice(split) + "after") + filter.finish(), "before" + QUERY_TERMINATOR + "after");
    }
    const filter = new TerminalQueryFilter();
    assert.equal([...query].map((byte) => filter.filter(byte)).join("") + filter.finish(), QUERY_TERMINATOR);
  }
});

test("unknown VT, drawing, keys and oversized candidates survive every split", () => {
  for (const text of agreement.passThrough) {
    for (let split = 0; split <= text.length; split += 1) {
      const filter = new TerminalQueryFilter();
      assert.equal(filter.filter(text.slice(0, split)) + filter.filter(text.slice(split)) + filter.finish(), text);
    }
  }
});

test("the complete Studio presentation path preserves ANSI, indexed and truecolor drawing at every split", () => {
  const colours = [
    "\x1b[31mred\x1b[0m", "\x1b[96;44mcyan\x1b[39;49m",
    "\x1b[38;5;208mindexed\x1b[0m", "\x1b[38;2;254;128;32mforeground\x1b[0m",
    "\x1b[48;2;12;34;56mbackground\x1b[0m", "\x1b[38:2::254:128:32mcolon\x1b[0m",
  ];
  for (const text of colours) {
    for (let split = 0; split <= text.length; split += 1) {
      const queries = new TerminalQueryFilter();
      const mouse = new MouseModeFilter();
      const output = mouse.filter(queries.filter(text.slice(0, split)))
        + mouse.filter(queries.filter(text.slice(split))) + mouse.filter(queries.finish()) + mouse.finish();
      assert.equal(output, text);
    }
  }
});

test("a cancelled query cannot consume a later query or unrelated drawing", () => {
  const filter = new TerminalQueryFilter();
  assert.equal(filter.filter("\x1bP+q123\x1b[6ntext"), "\x1bP+q123" + QUERY_TERMINATOR + "text");
  assert.equal(filter.filter("\x1b[?2"), "");
  assert.equal(filter.finish(), "\x1b[?2");
  assert.equal(filter.filter("026$p"), "026$p");
});

test("owned queries preserve the preceding CSI cancellation and OSC/DCS completion at every split", () => {
  for (const item of agreement.cancellation) {
    const bytes = item.before + "\x1b[6n" + item.after;
    for (let split = 0; split <= bytes.length; split += 1) {
      const filter = new TerminalQueryFilter();
      assert.equal(filter.filter(bytes.slice(0, split)) + filter.filter(bytes.slice(split)) + filter.finish(), item.before + QUERY_TERMINATOR + item.after);
    }
  }
});
