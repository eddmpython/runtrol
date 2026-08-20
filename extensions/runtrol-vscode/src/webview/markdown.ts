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

export type Block =
  | { kind: "paragraph"; inlines: Inline[] }
  | { kind: "heading"; level: number; inlines: Inline[] }
  | { kind: "list"; ordered: boolean; items: Inline[][] }
  /// `open` marks a fence whose closing line has not arrived yet, which is normal mid-stream.
  | { kind: "codeBlock"; language: string; text: string; open: boolean };

/// Whether this text can contain anything the grammar would change.
///
/// The plain path is the hot path: a reply with none of these characters is appended as one text
/// node exactly as before, so the renderer costs nothing when there is nothing to render.
const TRIGGER = /[`*#[]|(?:^|\n)\s{0,3}(?:-|\d+\.)\s/u;

export function hasMarkdownTrigger(text: string): boolean {
  return TRIGGER.test(text);
}

const FENCE_OPEN = /^```(\S*)\s*$/u;
const HEADING = /^(#{1,6})\s+(.*)$/u;
const BULLET = /^\s{0,3}[-*]\s+(.*)$/u;
const NUMBERED = /^\s{0,3}\d+\.\s+(.*)$/u;
const LINK_HREF = /^https?:\/\/\S+$/u;

export function parseMarkdown(text: string): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let list: { ordered: boolean; items: Inline[][] } | null = null;
  let fence: { language: string; lines: string[] } | null = null;

  const sealParagraph = (): void => {
    if (paragraph.length > 0) {
      blocks.push({ kind: "paragraph", inlines: inlinesOf(paragraph.join("\n")) });
      paragraph = [];
    }
  };
  const sealList = (): void => {
    if (list) {
      blocks.push({ kind: "list", ordered: list.ordered, items: list.items });
      list = null;
    }
  };

  for (const line of text.split("\n")) {
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
    // A bullet needs the space after its marker, so "*emphasis*" stays emphasis and "-x" stays text.
    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    const item = bullet ?? numbered;
    if (item) {
      sealParagraph();
      const ordered = numbered !== null;
      if (!list || list.ordered !== ordered) {
        sealList();
        list = { ordered, items: [] };
      }
      list.items.push(inlinesOf(item[1] ?? ""));
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
