/// This presentation grammar is checked against runtrol-terminal-protocol's agreement fixture.
/// The host owns replies. Consuming its queries before rendering keeps handleInput unambiguously user input.
export const OWNED_QUERY_LITERALS = ["[>0q", "[>q", "[6n", "[5n", "[0c", "[c", "[>0c", "[>c", "[?u"];
export const QUERY_CARRY_LIMIT = 128;
export const QUERY_MODE_DIGITS = 5;
/// Preserve the leading ESC's completion of earlier CSI/OSC/DCS while producing no reply or drawing.
export const QUERY_TERMINATOR = "\x1b\\";

type Scan = { kind: "query"; end: number } | { kind: "unfinished" } | { kind: "other" };

export class TerminalQueryFilter {
  private tail = "";

  filter(text: string): string {
    const window = this.tail + text;
    this.tail = "";
    let out = "";
    let at = 0;
    while (at < window.length) {
      if (window[at] !== "\x1b") {
        out += window[at];
        at += 1;
        continue;
      }
      const scanned = queryAt(window, at);
      if (scanned.kind === "query" && scanned.end - at <= QUERY_CARRY_LIMIT) {
        out += QUERY_TERMINATOR;
        at = scanned.end;
      } else if (scanned.kind === "unfinished" && window.length - at <= QUERY_CARRY_LIMIT) {
        this.tail = window.slice(at);
        return out;
      } else {
        out += window[at];
        at += 1;
      }
    }
    return out;
  }

  /// Unknown unfinished output remains literal at exit or before a replacement stream starts.
  finish(): string {
    const tail = this.tail;
    this.tail = "";
    return tail;
  }
}

function queryAt(window: string, start: number): Scan {
  const rest = window.slice(start + 1);
  const complete = (length: number): Scan => ({ kind: "query", end: start + 1 + length });
  if (rest.length === 0) return { kind: "unfinished" };
  let unfinished = false;
  for (const literal of OWNED_QUERY_LITERALS) {
    if (rest.startsWith(literal)) return complete(literal.length);
    unfinished ||= literal.startsWith(rest);
  }
  if (rest.startsWith("[?")) {
    const after = rest.slice(2);
    const digits = after.match(/^\d*/u)![0].length;
    if (digits > QUERY_MODE_DIGITS) return { kind: "other" };
    if (digits === 0) return { kind: after.length === 0 && unfinished ? "unfinished" : "other" };
    const suffix = after.slice(digits);
    if (suffix === "" || suffix === "$") return { kind: "unfinished" };
    return suffix.startsWith("$p") ? complete(2 + digits + 2) : { kind: "other" };
  }
  for (const prefix of ["]10;?", "]11;?"]) {
    if (rest.startsWith(prefix)) {
      const after = rest.slice(prefix.length);
      if (after === "" || after === "\x1b") return { kind: "unfinished" };
      if (after.startsWith("\x07")) return complete(prefix.length + 1);
      if (after.startsWith("\x1b\\")) return complete(prefix.length + 2);
      return { kind: "other" };
    }
    unfinished ||= prefix.startsWith(rest);
  }
  if (rest.startsWith("P+q")) {
    const after = rest.slice(3);
    const end = after.indexOf("\x1b");
    if (!/^[0-9a-f;]*$/iu.test(end < 0 ? after : after.slice(0, end))) return { kind: "other" };
    if (end >= 0) {
      return after.slice(end).startsWith("\x1b\\") ? complete(3 + end + 2)
        : after.length === end + 1 ? { kind: "unfinished" } : { kind: "other" };
    }
    return { kind: "unfinished" };
  }
  unfinished ||= "P+q".startsWith(rest);
  return { kind: unfinished ? "unfinished" : "other" };
}
