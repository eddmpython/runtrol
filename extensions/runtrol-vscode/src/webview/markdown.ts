/// A deliberately small markdown reading of an agent's reply.
///
/// DOM-free: this module turns text into descriptors and the page materialises them with
/// `createElement`/`createTextNode`, so no string of conversation ever becomes HTML. That is also why
/// no library is used here: the maintained ones produce HTML strings, and rendering those requires
/// `innerHTML`, which this webview bans.
///
/// The grammar is the part of markdown coding agents actually emit: fenced code, inline code,
/// headings, flat lists, bold, italic, and http(s) links. Everything else stays exactly as typed.
/// A wrong guess here is only a display preference, never a safety problem, because every leaf is a
/// text node either way.

export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; text: string }
  | { kind: "em"; text: string }
  | { kind: "link"; text: string; href: string };

/// One entry of a list, and the list written underneath it when the author indented one there.
export type ListItem = {
  inlines: Inline[];
  list: ListBlock | null;
};

export type ListBlock = { kind: "list"; ordered: boolean; items: ListItem[] };

export type Block =
  | { kind: "paragraph"; inlines: Inline[] }
  | { kind: "heading"; level: number; inlines: Inline[] }
  | ListBlock
  | { kind: "quote"; inlines: Inline[] }
  /// A table keeps its shape: the header cells, then a row of cells per line.
  | { kind: "table"; head: Inline[][]; rows: Inline[][][] }
  /// `open` marks a fence whose closing line has not arrived yet, which is normal mid-stream.
  | { kind: "codeBlock"; language: string; text: string; open: boolean };

/// Whether this text can contain anything the grammar would change.
///
/// The plain path is the hot path: a reply with none of these characters is appended as one text
/// node exactly as before, so the renderer costs nothing when there is nothing to render.
const TRIGGER = /[`*#[]|(?:^|\n)\s*(?:[->]\s|\d+\.\s|\|)/u;

export function hasMarkdownTrigger(text: string): boolean {
  return TRIGGER.test(text);
}

const FENCE_OPEN = /^```(\S*)\s*$/u;
const HEADING = /^(#{1,6})\s+(.*)$/u;
const BULLET = /^(\s*)[-*]\s+(.*)$/u;
const NUMBERED = /^(\s*)\d+\.\s+(.*)$/u;
const QUOTE = /^\s*>\s?(.*)$/u;
/// A table line is a row of cells between pipes; the line under the header is only dashes and colons.
const TABLE_ROW = /^\s*\|(.*)\|\s*$/u;
const TABLE_RULE = /^\s*\|(?:\s*:?-{1,}:?\s*\|)+\s*$/u;
const LINK_HREF = /^https?:\/\/\S+$/u;

/// One open list at one indentation. A deeper line opens a frame under the item above it, which is how a list
/// written inside a list keeps its shape instead of arriving as one flat run of entries.
type ListFrame = { indent: number; block: ListBlock };

export function parseMarkdown(text: string): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  const frames: ListFrame[] = [];
  let fence: { language: string; lines: string[] } | null = null;

  const sealParagraph = (): void => {
    if (paragraph.length > 0) {
      blocks.push({ kind: "paragraph", inlines: inlinesOf(paragraph.join("\n")) });
      paragraph = [];
    }
  };
  const sealList = (): void => {
    // Only the outermost list is a block of its own. The deeper ones are already held by their own items.
    const root = frames[0];
    if (root) blocks.push(root.block);
    // Emptied in place rather than replaced, because the placer holds this same array.
    frames.length = 0;
  };

  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    if (fence) {
      if (line.trimEnd() === "```") {
        blocks.push({ kind: "codeBlock", language: fence.language, text: fence.lines.join("\n"), open: false });
        fence = null;
      } else {
        fence.lines.push(line);
      }
      continue;
    }
    const opened = FENCE_OPEN.exec(line);
    if (opened) {
      sealParagraph();
      sealList();
      fence = { language: opened[1] ?? "", lines: [] };
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      sealParagraph();
      sealList();
      blocks.push({ kind: "heading", level: heading[1]?.length ?? 1, inlines: inlinesOf(heading[2] ?? "") });
      continue;
    }
    // A header row is only a table once the line under it is the rule. Without that a line of pipes is a
    // sentence someone happened to write with pipes in it.
    if (TABLE_ROW.test(line) && TABLE_RULE.test(lines[index + 1] ?? "")) {
      sealParagraph();
      sealList();
      const head = cellsOf(line);
      const rows: Inline[][][] = [];
      let cursor = index + 2;
      while (cursor < lines.length && TABLE_ROW.test(lines[cursor] ?? "")) {
        rows.push(cellsOf(lines[cursor] ?? ""));
        cursor += 1;
      }
      blocks.push({ kind: "table", head, rows });
      index = cursor - 1;
      continue;
    }
    const quoted = QUOTE.exec(line);
    if (quoted) {
      sealParagraph();
      sealList();
      const quotedLines = [quoted[1] ?? ""];
      let cursor = index + 1;
      for (;;) {
        const next = QUOTE.exec(lines[cursor] ?? "");
        if (!next) break;
        quotedLines.push(next[1] ?? "");
        cursor += 1;
      }
      blocks.push({ kind: "quote", inlines: inlinesOf(quotedLines.join("\n")) });
      index = cursor - 1;
      continue;
    }
    // A bullet needs the space after its marker, so "*emphasis*" stays emphasis and "-x" stays text.
    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    const item = bullet ?? numbered;
    if (item) {
      sealParagraph();
      addListItem(frames, (item[1] ?? "").length, numbered !== null, inlinesOf(item[2] ?? ""), sealList);
      continue;
    }
    if (!line.trim()) {
      sealParagraph();
      sealList();
      continue;
    }
    sealList();
    paragraph.push(line);
  }
  sealParagraph();
  sealList();
  if (fence) {
    blocks.push({ kind: "codeBlock", language: fence.language, text: fence.lines.join("\n"), open: true });
  }
  return blocks;
}

/// Place one entry at its indentation, opening or closing the lists between it and the entry before it.
function addListItem(
  frames: ListFrame[],
  indent: number,
  ordered: boolean,
  inlines: Inline[],
  sealList: () => void,
): void {
  while (frames.length > 1 && indent < (frames[frames.length - 1]?.indent ?? 0)) frames.pop();
  const top = frames[frames.length - 1];
  if (!top) {
    frames.push({ indent, block: { kind: "list", ordered, items: [] } });
  } else if (indent > top.indent) {
    const parent = top.block.items[top.block.items.length - 1];
    const block: ListBlock = { kind: "list", ordered, items: [] };
    // Indented under nothing is still an entry of the list above it rather than a lost line.
    if (parent) {
      parent.list = block;
      frames.push({ indent, block });
    }
  } else if (top.block.ordered !== ordered) {
    // Numbers after bullets at the same level are a different list, not a continuation of this one.
    sealList();
    frames.push({ indent, block: { kind: "list", ordered, items: [] } });
  }
  const current = frames[frames.length - 1];
  if (current) current.block.items.push({ inlines, list: null });
}

/// The cells of one table line, without the outer pipes.
function cellsOf(line: string): Inline[][] {
  const inner = TABLE_ROW.exec(line)?.[1] ?? "";
  return inner.split("|").map((cell) => inlinesOf(cell.trim()));
}

/// Inline spans, one pass, code first so nothing formats inside a code span.
///
/// An unclosed marker is not an error: the rest of the line stays plain text, which is what a person
/// watching a stream expects while the closing character is still on its way.
function inlinesOf(text: string): Inline[] {
  const out: Inline[] = [];
  let plain = "";
  const flush = (): void => {
    if (plain) {
      out.push({ kind: "text", text: plain });
      plain = "";
    }
  };
  let index = 0;
  while (index < text.length) {
    const ch = text[index];
    if (ch === "`") {
      const end = text.indexOf("`", index + 1);
      if (end > index) {
        flush();
        out.push({ kind: "code", text: text.slice(index + 1, end) });
        index = end + 1;
        continue;
      }
    }
    if (ch === "*" && text[index + 1] === "*") {
      const end = text.indexOf("**", index + 2);
      if (end > index + 1) {
        flush();
        out.push({ kind: "strong", text: text.slice(index + 2, end) });
        index = end + 2;
        continue;
      }
    }
    if (ch === "*") {
      const end = text.indexOf("*", index + 1);
      if (end > index + 1) {
        flush();
        out.push({ kind: "em", text: text.slice(index + 1, end) });
        index = end + 1;
        continue;
      }
    }
    if (ch === "[") {
      const close = text.indexOf("](", index + 1);
      const end = close > index ? text.indexOf(")", close + 2) : -1;
      if (close > index && end > close) {
        const href = text.slice(close + 2, end);
        // Only web links become anchors. Anything else (command:, file:, javascript:) stays text,
        // because a link is a navigation this page hands to the host and the host must only ever
        // receive addresses a browser can hold.
        if (LINK_HREF.test(href)) {
          flush();
          out.push({ kind: "link", text: text.slice(index + 1, close) || href, href });
          index = end + 1;
          continue;
        }
      }
    }
    plain += ch;
    index += 1;
  }
  flush();
  return out;
}
