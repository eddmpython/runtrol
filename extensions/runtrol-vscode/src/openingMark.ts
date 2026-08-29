/// The Runtrol mark, turning in the middle of a conversation tab while it opens.
///
/// # Why anything is drawn at all
///
/// Opening a conversation asks the Runtime for a terminal, which starts or attaches to a coding CLI. That is
/// fast but it is not instant, and until the CLI writes its first byte the tab is an empty black rectangle.
/// An empty rectangle says the same thing as a broken one, so the person waits without knowing whether they
/// are waiting (operator, 2026-08-28: show the symbol moving, like a spinner).
///
/// # Why these characters
///
/// The mark is four curved arms with rotational symmetry (`resources/symbol.svg`). A terminal has no drawing
/// surface, only characters, and the four rounded box elbows are those same four arms: turning the block a
/// quarter turn moves each elbow to the next corner, which is what the mark does when it spins. A dot spinner
/// would have been anyone's; this one is ours (operator, 2026-08-28: show our own symbol and make it move).

/// The four corners, clockwise from the top left, in each of the four quarter turns.
/// The mark itself, standing still: the four arms in their corners, top to bottom. It does not spin
/// (operator, 2026-08-29: use our symbol as it is and put the effect on it, not a rotation). The light is
/// what moves; the symbol is the thing the light moves across.
const SYMBOL: readonly string[] = [
  "╭   ╮",
  "     ",
  "╰   ╯",
];

const BLOCK_ROWS = SYMBOL.length;
const BLOCK_COLUMNS = SYMBOL[0]?.length ?? 0;

/// The mark as its plain lines, the same every frame. Kept exported so a test can read the shape without the
/// escape sequences the light adds.
export function markFrame(): readonly string[] {
  return SYMBOL;
}

/// The Runtrol coral, as a truecolor foreground. The light that sweeps the symbol is brand-coloured, so the
/// motion reads as our light rather than as the terminal blinking.
const LIT = "\x1b[1;38;2;245;101;101m";
const NEAR = "\x1b[0m";
const DIM = "\x1b[2m";

/// The sweep runs one column past the symbol on each side, so the light fully enters and fully leaves before
/// it comes round again. That gap is what makes the motion read as a repeating left-to-right pass rather than
/// a column that just jumps back.
const SWEEP_MARGIN = 2;
const SWEEP_PERIOD = BLOCK_COLUMNS + SWEEP_MARGIN * 2;

/// The escape for one cell at column `x`, given where the light is (`lit`): brightest under the light, normal
/// one column to either side, dim elsewhere. This is the whole effect, per character.
function cellStyle(x: number, lit: number): string {
  const distance = Math.abs(x - lit);
  if (distance === 0) return LIT;
  if (distance === 1) return NEAR;
  return DIM;
}

/// The escape sequence that paints one frame centred in a terminal of this size.
///
/// The symbol stands still and a coral light sweeps across it left to right, over and over. The whole screen
/// is cleared each frame rather than only the block: the terminal this draws into is about to be handed to a
/// CLI that will clear it anyway, and a partial repaint of a pane that may have been resized leaves the old
/// block behind.
export function paintMark(at: number, columns: number, rows: number): string {
  const top = Math.max(1, Math.floor((rows - BLOCK_ROWS) / 2));
  const left = Math.max(1, Math.floor((columns - BLOCK_COLUMNS) / 2));
  const lit = (at % SWEEP_PERIOD) - SWEEP_MARGIN;
  const painted = SYMBOL
    .map((line, row) => {
      const cells = [...line]
        .map((character, x) => `${cellStyle(x, lit)}${character}`)
        .join("");
      return `\x1b[${top + row};${left}H${cells}\x1b[0m`;
    })
    .join("");
  return `\x1b[2J${painted}`;
}

/// How often a frame is drawn. The light moves one column per frame, and at this interval its pass across the
/// symbol reads as motion without asking the terminal to repaint faster than it can.
export const MARK_FRAME_MS = 110;

/// Hide the cursor while the mark turns; a block cursor parked at the top left is not part of the picture.
export const HIDE_CURSOR = "\x1b[?25l";
export const SHOW_CURSOR = "\x1b[?25h";

/// Whether a chunk the service wrote puts anything on the screen a person could see.
///
/// Escape sequences move the cursor, clear the screen and set colours without drawing a mark, and a screen
/// handed back for a conversation that has not started is exactly that. The scan is deliberately shallow: it
/// removes the escape sequences and asks whether any non-blank character is left. It never looks at what the
/// characters say.
export function hasVisibleText(text: string): boolean {
  let visible = false;
  let inEscape = false;
  for (const character of text) {
    if (character === "\x1b") {
      inEscape = true;
      continue;
    }
    if (inEscape) {
      // A control sequence ends at its final byte; everything before it only steers the terminal.
      if (character >= "@" && character <= "~") inEscape = false;
      continue;
    }
    if (character > " ") visible = true;
  }
  return visible;
}
