import assert from "node:assert/strict";
import test from "node:test";

import { hasMarkdownTrigger, parseMarkdown, type Block, type ListBlock } from "./markdown";

test("the trigger is absent from ordinary streamed text", () => {
  assert.equal(hasMarkdownTrigger("frame 12\nframe 13\n"), false);
  assert.equal(hasMarkdownTrigger("The tests pass now."), false);
  assert.equal(hasMarkdownTrigger("a - b"), false, "a dash inside a sentence is not a list");
});

test("the trigger fires on everything the grammar reads", () => {
  for (const sample of ["`code`", "**bold**", "# Heading", "- item", "1. item", "[t](https://x)"]) {
    assert.equal(hasMarkdownTrigger(sample), true, sample);
  }
});

test("a fenced block seals on its closing line and stays open without one", () => {
  const sealed = parseMarkdown("before\n```rust\nfn main() {}\n```\nafter");
  assert.deepEqual(sealed, [
    { kind: "paragraph", inlines: [{ kind: "text", text: "before" }] },
    { kind: "codeBlock", language: "rust", text: "fn main() {}", open: false },
    { kind: "paragraph", inlines: [{ kind: "text", text: "after" }] },
  ]);
  const streaming = parseMarkdown("```\nhalf a block");
  assert.deepEqual(streaming, [{ kind: "codeBlock", language: "", text: "half a block", open: true }]);
});

test("markdown inside a fence stays code", () => {
  const blocks = parseMarkdown("```\n# not a heading\n- not a list\n```");
  assert.deepEqual(blocks, [
    { kind: "codeBlock", language: "", text: "# not a heading\n- not a list", open: false },
  ]);
});

test("headings, lists, and paragraphs split where a person expects", () => {
  const blocks = parseMarkdown("## Plan\n- first\n- second\n1. then\n\ntext line one\ntext line two");
  assert.deepEqual(blocks, [
    { kind: "heading", level: 2, inlines: [{ kind: "text", text: "Plan" }] },
    {
      kind: "list",
      ordered: false,
      items: [
        { inlines: [{ kind: "text", text: "first" }], list: null },
        { inlines: [{ kind: "text", text: "second" }], list: null },
      ],
    },
    { kind: "list", ordered: true, items: [{ inlines: [{ kind: "text", text: "then" }], list: null }] },
    { kind: "paragraph", inlines: [{ kind: "text", text: "text line one\ntext line two" }] },
  ]);
});

test("inline spans read code first and leave unclosed markers as text", () => {
  const [paragraph] = parseMarkdown("say `let *x*` and **loud** and *soft* and *dangling");
  assert.ok(paragraph?.kind === "paragraph");
  assert.deepEqual(paragraph.inlines, [
    { kind: "text", text: "say " },
    { kind: "code", text: "let *x*" },
    { kind: "text", text: " and " },
    { kind: "strong", text: "loud" },
    { kind: "text", text: " and " },
    { kind: "em", text: "soft" },
    { kind: "text", text: " and *dangling" },
  ]);
});

test("only web addresses become links", () => {
  const [good] = parseMarkdown("[docs](https://example.com/a)");
  assert.ok(good?.kind === "paragraph");
  assert.deepEqual(good.inlines, [{ kind: "link", text: "docs", href: "https://example.com/a" }]);
  for (const hostile of ["[x](javascript:alert(1))", "[x](command:doThing)", "[x](file:///etc/passwd)"]) {
    const [block] = parseMarkdown(hostile);
    assert.ok(block?.kind === "paragraph");
    assert.ok(
      block.inlines.every((inline) => inline.kind === "text"),
      `${hostile} must stay plain text`,
    );
  }
});

test("a line starting with emphasis is not a bullet", () => {
  const [block] = parseMarkdown("*emphasis* leads this line");
  assert.ok(block?.kind === "paragraph");
  assert.deepEqual(block.inlines[0], { kind: "em", text: "emphasis" });
});

test("a list written under an entry stays under it", () => {
  const [list] = parseMarkdown("- outer one\n  - inner a\n  - inner b\n- outer two");
  assert.ok(list?.kind === "list");
  assert.deepEqual(list.items.map((item) => item.inlines), [
    [{ kind: "text", text: "outer one" }],
    [{ kind: "text", text: "outer two" }],
  ], "only the outer entries are entries of the outer list");
  assert.deepEqual(list.items[0]?.list?.items.map((item) => item.inlines), [
    [{ kind: "text", text: "inner a" }],
    [{ kind: "text", text: "inner b" }],
  ], "the indented pair belongs to the entry above them");
  assert.equal(list.items[1]?.list, null, "the entry with nothing under it carries no list");
});

test("a numbered list indented under a bullet keeps both kinds", () => {
  const [list] = parseMarkdown("- step\n  1. first\n  2. second");
  assert.ok(list?.kind === "list");
  assert.equal(list.ordered, false, "the outer list is the bullet one");
  assert.equal(list.items[0]?.list?.ordered, true, "the list under it is the numbered one");
});

test("a table keeps its header and its rows", () => {
  const [table] = parseMarkdown("| name | count |\n| --- | ---: |\n| alpha | 1 |\n| beta | 2 |");
  assert.ok(table?.kind === "table");
  assert.deepEqual(table.head, [
    [{ kind: "text", text: "name" }],
    [{ kind: "text", text: "count" }],
  ]);
  assert.deepEqual(table.rows.map((row) => row.map((cell) => cell.map((span) => span.text).join(""))), [
    ["alpha", "1"],
    ["beta", "2"],
  ]);
});

test("pipes are only a table when the line under the header is the rule", () => {
  // Lines that are already shaped like table rows, so the rule beneath the first is the only difference.
  const withoutRule = parseMarkdown("| name | count |\n| alpha | 1 |");
  assert.deepEqual(
    withoutRule.map((block) => block.kind),
    ["paragraph"],
    "rows with no rule under the header stay a paragraph",
  );
  const withRule = parseMarkdown("| name | count |\n| --- | --- |\n| alpha | 1 |");
  assert.deepEqual(withRule.map((block) => block.kind), ["table"], "the same rows with the rule are a table");
  const sentence = parseMarkdown("run a | b | c\nand then more");
  assert.deepEqual(sentence.map((block) => block.kind), ["paragraph"], "a sentence with pipes stays a sentence");
});

test("a quoted run becomes one quote", () => {
  const blocks = parseMarkdown("> first line\n> second line\n\nafter");
  assert.ok(blocks[0]?.kind === "quote");
  assert.deepEqual(blocks[0].inlines, [{ kind: "text", text: "first line\nsecond line" }]);
  assert.equal(blocks[1]?.kind, "paragraph", "the text after the quote is its own paragraph");
});

test("what the renderer is asked to read is what the trigger admits", () => {
  for (const sample of ["- a", "  - a", "1. a", "> a", "| a | b |", "**a**", "`a`", "# a"]) {
    assert.equal(hasMarkdownTrigger(sample), true, `${sample} reaches the markdown path`);
  }
  for (const sample of ["plain words", "a-b c", "3.5 apples"]) {
    assert.equal(hasMarkdownTrigger(sample), false, `${sample} stays on the plain path`);
  }
});

/// Every entry the descriptor tree holds, in order, however deep it sits.
function everyItem(blocks: readonly Block[]): string[] {
  const out: string[] = [];
  const walk = (list: ListBlock): void => {
    for (const item of list.items) {
      out.push(item.inlines.map((span) => span.text).join(""));
      if (item.list) walk(item.list);
    }
  };
  for (const block of blocks) if (block.kind === "list") walk(block);
  return out;
}

test("a list keeps every entry when the indentation steps back to a depth nobody opened", () => {
  // Measured cases: an entry already carrying a deeper list used to have that list replaced, and everything
  // written under it disappeared from the conversation.
  const mixed = parseMarkdown("- Setup\n    - install deps\n  - configure\n- Run");
  assert.deepEqual(everyItem(mixed), ["Setup", "install deps", "configure", "Run"]);

  const deeper = parseMarkdown("- a\n    - b\n        - c\n  - d\n- e");
  assert.deepEqual(everyItem(deeper), ["a", "b", "c", "d", "e"]);

  const ragged = parseMarkdown(" - a\n      - b\n   - c\n    - d\n  - e");
  assert.deepEqual(everyItem(ragged), ["a", "b", "c", "d", "e"]);
});

test("no arrangement of indents loses an entry", () => {
  // Every entry written must be an entry read back, whatever the indents do between them.
  const depths = [0, 2, 4, 6];
  for (let seed = 0; seed < 4096; seed += 1) {
    const lines: string[] = [];
    const expected: string[] = [];
    let value = seed;
    for (let index = 0; index < 6; index += 1) {
      const depth = depths[value % depths.length] ?? 0;
      value = Math.floor(value / depths.length);
      const label = `i${index}`;
      lines.push(`${" ".repeat(depth)}- ${label}`);
      expected.push(label);
    }
    assert.deepEqual(
      everyItem(parseMarkdown(lines.join("\n"))),
      expected,
      `these indents lost an entry:\n${lines.join("\n")}`,
    );
  }
});
