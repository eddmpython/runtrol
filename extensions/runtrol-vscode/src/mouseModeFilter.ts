/// DEC private modes that switch a terminal's mouse reporting on or off, in every flavour a CLI sends: X10 (9),
/// normal (1000), highlight (1001), button motion (1002), any motion (1003), and the UTF-8, SGR, urxvt and pixel
/// encodings (1005, 1006, 1015, 1016).
const MOUSE_MODES = new Set([9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]);

/// How much of an unfinished sequence is carried to the next chunk. A stray ESC never grows the carry past this.
const CARRY_LIMIT = 24;

/// The one exception the transparent terminal design names, at the viewer's own edge.
///
/// The Core forwards the CLI's bytes exactly as the host read them, mouse-mode switches included (the raw lane
/// rewrites nothing). This tab is a VS Code terminal that keeps its own mouse: selecting on drag, scrolling on
/// wheel, no click ever reaching the CLI (operator, 2026-08-29). So the exact control family that would put the
/// tab's terminal into mouse mode is taken out here, in the client, before the bytes reach VS Code. Other private
/// modes in the same sequence (`ESC [ ? 1049 ; 1000 h`) are kept; only the mouse parameters leave. A sequence
/// split across two writes is carried to the next one, bounded. The checkpoint a late viewer receives passes
/// through the same filter, because the screen model replays the modes it holds.
///
/// Nothing else is touched: keys, paste, IME text, and every other byte the CLI drew are the CLI's.
export class MouseModeFilter {
  private tail = "";

  /// `text` with the mouse-mode switches removed, plus whatever the previous chunk left unfinished.
  filter(text: string): string {
    const window = this.tail + text;
    this.tail = "";
    let out = "";
    let at = 0;
    while (at < window.length) {
      const character = window[at] as string;
      if (character !== "\x1b") {
        out += character;
        at += 1;
        continue;
      }
      const scanned = privateModeAt(window, at);
      if (scanned.kind === "complete") {
        const kept = scanned.params.filter((mode) => !MOUSE_MODES.has(mode));
        if (kept.length === scanned.params.length) {
          out += window.slice(at, scanned.end);
        } else if (kept.length > 0) {
          out += `\x1b[?${kept.join(";")}${scanned.action}`;
        }
        at = scanned.end;
      } else if (scanned.kind === "incomplete") {
        const rest = window.slice(at);
        if (rest.length <= CARRY_LIMIT) {
          this.tail = rest;
        } else {
          out += rest;
        }
        return out;
      } else {
        out += character;
        at += 1;
      }
    }
    return out;
  }

  /// Forget an unfinished sequence: the stream restarts (a replacement checkpoint, a reattached view).
  reset(): void {
    this.tail = "";
  }
}

type ModeScan =
  | { kind: "complete"; end: number; params: number[]; action: "h" | "l" }
  | { kind: "incomplete" }
  | { kind: "other" };

/// What begins at `start` (an ESC), if it is a DEC private mode set or reset.
function privateModeAt(window: string, start: number): ModeScan {
  const rest = window.slice(start + 1);
  if (rest.length === 0) return { kind: "incomplete" };
  if (!rest.startsWith("[?")) {
    // `ESC [` alone may still become `ESC [ ?` on the next write.
    return rest === "[" ? { kind: "incomplete" } : { kind: "other" };
  }
  const after = rest.slice(2);
  let body = 0;
  while (body < after.length && /[0-9;]/u.test(after[body] as string)) body += 1;
  const action = after[body];
  if ((action === "h" || action === "l") && body > 0) {
    const params: number[] = [];
    for (const piece of after.slice(0, body).split(";")) {
      // A parameter this does not read as a number is not one it may rewrite.
      if (!/^\d{1,5}$/u.test(piece)) return { kind: "other" };
      params.push(Number(piece));
    }
    return { kind: "complete", end: start + 3 + body + 1, params, action };
  }
  if (action === undefined && body < 16) return { kind: "incomplete" };
  return { kind: "other" };
}
