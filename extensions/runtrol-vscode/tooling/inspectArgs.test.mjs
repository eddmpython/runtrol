import assert from "node:assert/strict";
import test from "node:test";

import { parseInspectArgs } from "./inspectArgs.mjs";

test("a subcommand is required and must be known", () => {
  assert.match(parseInspectArgs([]).error, /subcommand is required/);
  assert.match(parseInspectArgs(["look"]).error, /unknown subcommand/);
});

test("the title defaults to every VS Code window and can be narrowed", () => {
  assert.deepEqual(parseInspectArgs(["list"]), { subcommand: "list", title: "Visual Studio Code", command: "" });
  assert.deepEqual(
    parseInspectArgs(["list", "--title", "runtrol-eye", "--command", "user-data-dir=abc"]),
    { subcommand: "list", title: "runtrol-eye", command: "user-data-dir=abc" },
  );
  assert.match(parseInspectArgs(["list", "--title", ""]).error, /must not be empty/);
});

test("capture carries its output path and the opt-in front flag", () => {
  assert.deepEqual(
    parseInspectArgs(["capture"]),
    { subcommand: "capture", title: "Visual Studio Code", command: "", out: null, front: false },
  );
  const withOut = parseInspectArgs(["capture", "--out", "shot.png", "--front"]);
  assert.equal(withOut.out, "shot.png");
  assert.equal(withOut.front, true);
});

test("keys refuses to run with nothing to type", () => {
  assert.match(parseInspectArgs(["keys"]).error, /needs --keys/);
  assert.deepEqual(
    parseInspectArgs(["keys", "--keys", "^k^b"]),
    { subcommand: "keys", title: "Visual Studio Code", command: "", keys: "^k^b" },
  );
});

test("click requires non-negative integer client coordinates", () => {
  assert.match(parseInspectArgs(["click"]).error, /--x and --y/);
  assert.match(parseInspectArgs(["click", "--x", "-1", "--y", "0"]).error, /non-negative/);
  assert.match(parseInspectArgs(["click", "--x", "1.5", "--y", "2"]).error, /non-negative/);
  assert.deepEqual(
    parseInspectArgs(["click", "--x", "120", "--y", "240"]),
    { subcommand: "click", title: "Visual Studio Code", command: "", x: 120, y: 240 },
  );
});

test("a flag with no value, and a stray argument, are refused rather than half-read", () => {
  assert.match(parseInspectArgs(["capture", "--out"]).error, /needs a value/);
  assert.match(parseInspectArgs(["capture", "positional"]).error, /unexpected argument/);
});
