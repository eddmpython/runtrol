import assert from "node:assert/strict";
import test from "node:test";

import { hasMarkdownTrigger, parseMarkdown } from "./markdown";

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
      items: [[{ kind: "text", text: "first" }], [{ kind: "text", text: "second" }]],
    },
    { kind: "list", ordered: true, items: [[{ kind: "text", text: "then" }]] },
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
