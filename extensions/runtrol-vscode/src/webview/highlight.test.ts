import assert from "node:assert/strict";
import test from "node:test";

import { highlight, isKnownLanguage, type Token, type TokenKind } from "./highlight";

function joined(tokens: readonly Token[]): string {
  return tokens.map((token) => token.text).join("");
}

function kindsOf(tokens: readonly Token[], kind: TokenKind): string[] {
  return tokens.filter((token) => token.kind === kind).map((token) => token.text);
}

/// The invariant that makes a wrong guess cost a colour and never a character.
const SAMPLES: ReadonlyArray<readonly [string, string]> = [
  ["ts", "const x = 1; // note\nfunction go(a: string) { return `hi ${a}`; }\n"],
  ["python", "def go(n):\n    # a note\n    return f'{n}' if n else None\n"],
  ["rust", "fn main() { /* block */ let v: Vec<u8> = vec![1, 2]; println!(\"hi\"); }\n"],
  ["bash", "#!/usr/bin/env bash\nset -e\nfor f in *.ts; do echo \"$f\"; done\n"],
  ["json", "{\"a\": 1, \"b\": [true, null], \"c\": \"x\"}\n"],
  ["", "no language here 'quoted' # hash\n"],
  ["cobol", "IDENTIFICATION DIVISION.\n"],
  ["ts", "unterminated 'string\nnext line\n"],
  ["ts", "/* never closed\n"],
  ["", ""],
];

test("colouring never changes a character of the code", () => {
  for (const [language, source] of SAMPLES) {
    assert.equal(joined(highlight(language, source)), source, `${language || "plain"} survived unchanged`);
  }
});

test("a run of code is split into non-empty tokens", () => {
  for (const [language, source] of SAMPLES) {
    for (const token of highlight(language, source)) {
      assert.notEqual(token.text, "", `${language || "plain"} produced no empty token`);
    }
  }
});

test("each language's own words are the ones coloured", () => {
  const ts = highlight("ts", "const done = await run(\"x\"); // tail");
  assert.ok(kindsOf(ts, "keyword").includes("const"), "const reads as a keyword");
  assert.ok(kindsOf(ts, "keyword").includes("await"), "await reads as a keyword");
  assert.ok(kindsOf(ts, "function").includes("run"), "a called name reads as a call");
  assert.deepEqual(kindsOf(ts, "string"), ["\"x\""], "the quoted argument is the string");
  assert.deepEqual(kindsOf(ts, "comment"), ["// tail"], "the trailing note is the comment");

  const py = highlight("python", "def go(): return None  # done");
  assert.ok(kindsOf(py, "keyword").includes("def"), "def reads as a keyword");
  assert.ok(kindsOf(py, "type").includes("None"), "None reads as a value word");
  assert.deepEqual(kindsOf(py, "comment"), ["# done"], "the hash note is the comment");
  // The other language's marker is not this language's marker.
  assert.deepEqual(kindsOf(highlight("ts", "a # b"), "comment"), [], "a hash is not a comment in TypeScript");
});

test("a language with no grammar still finds its strings and notes, and claims no keywords", () => {
  const tokens = highlight("cobol", "MOVE \"x\" TO Y. # note");
  assert.deepEqual(kindsOf(tokens, "string"), ["\"x\""], "the quoted run is still a string");
  assert.deepEqual(kindsOf(tokens, "comment"), ["# note"], "the note is still a note");
  assert.deepEqual(kindsOf(tokens, "keyword"), [], "no word is claimed as a keyword");
  assert.equal(isKnownLanguage("cobol"), false, "and the language is reported as unknown");
  assert.equal(isKnownLanguage("TypeScript"), true, "a known name is found whatever its case");
});

test("a number is a number only where one can begin", () => {
  assert.deepEqual(kindsOf(highlight("ts", "const a1 = 0xff + 2;"), "number"), ["0xff", "2"]);
  // A name that carries digits stays one whole name: no number is cut out of it, and the name is still there.
  const tokens = highlight("ts", "const a1 = 2;");
  assert.deepEqual(kindsOf(tokens, "number"), ["2"], "only the standalone digit is a number");
  assert.ok(
    tokens.some((token) => token.kind === "plain" && token.text.includes("a1")),
    "the name keeps its digit rather than being split around it",
  );
});

test("an unterminated string stops at the line, so the rest of the block keeps its colours", () => {
  const tokens = highlight("ts", "'open\nconst after = 1;\n");
  assert.deepEqual(kindsOf(tokens, "string"), ["'open"], "the string ends with its line");
  assert.ok(kindsOf(tokens, "keyword").includes("const"), "the next line is read normally");
});

test("a template literal spans lines, because that is what it does", () => {
  const tokens = highlight("ts", "const a = `one\ntwo`;\n");
  assert.deepEqual(kindsOf(tokens, "string"), ["`one\ntwo`"], "the backtick run keeps both lines");
});
