/// The Runtrol mark, turning in the middle of a conversation tab while it opens.
///
/// # Why anything is drawn at all
///
/// Opening a conversation asks the Runtime for a terminal, which starts or attaches to a coding CLI. That is
/// fast but it is not instant, and until the CLI writes its first byte the tab is an empty black rectangle.
/// An empty rectangle says the same thing as a broken one, so the person waits without knowing whether they
/// are waiting (operator, 2026-08-28: show the symbol moving, like a spinner).
///
/// # Why our own mark rather than a stock spinner
///
/// The tab belongs to a conversation Runtrol opened, and the waiting is Runtrol's. A dot spinner is anyone's;
/// the four-pointed mark with a dot going around it is the same shape as the ring the sidebar draws around a
/// running conversation, so a person meets one idea twice rather than two ideas once.

/// The mark itself, centred, with one of the eight positions around it lit.
const MARK = "✦";
const DOT = "·";
const LIT = "•";

/// The eight places the travelling dot can be, clockwise from the top, as offsets in the 3x5 block below.
const ORBIT: ReadonlyArray<readonly [number, number]> = [
  [0, 2], [0, 4], [1, 4], [2, 4], [2, 2], [2, 0], [1, 0], [0, 0],
];

const BLOCK_ROWS = 3;
const BLOCK_COLUMNS = 5;

/// One frame of the animation as the lines it draws, top to bottom.
export function markFrame(at: number): string[] {
  const lit = ORBIT[at % ORBIT.length];
  const rows: string[] = [];
  for (let row = 0; row < BLOCK_ROWS; row += 1) {
    let line = "";
    for (let column = 0; column < BLOCK_COLUMNS; column += 1) {
      if (row === 1 && column === 2) {
        line += MARK;
        continue;
      }
      const orbits = ORBIT.some(([r, c]) => r === row && c === column);
      if (!orbits) {
        line += " ";
        continue;
      }
      line += lit !== undefined && lit[0] === row && lit[1] === column ? LIT : DOT;
    }
    rows.push(line);
  }
  return rows;
}

/// The escape sequence that paints one frame centred in a terminal of this size.
///
/// The whole screen is cleared each frame rather than only the block: the terminal this draws into is about to
/// be handed to a CLI that will clear it anyway, and a partial repaint of a pane that may have been resized
/// leaves the old block behind.
export function paintMark(at: number, columns: number, rows: number): string {
  const lines = markFrame(at);
  const top = Math.max(1, Math.floor((rows - BLOCK_ROWS) / 2));
  const left = Math.max(1, Math.floor((columns - BLOCK_COLUMNS) / 2));
  const painted = lines
    .map((line, index) => `\x1b[${top + index};${left}H\x1b[2m${line}\x1b[0m`)
    .join("");
  return `\x1b[2J${painted}`;
}

/// How often a frame is drawn. Eight positions at this interval is one turn a second, which reads as motion
/// without asking the terminal to repaint faster than it can.
export const MARK_FRAME_MS = 125;

/// Hide the cursor while the mark turns; a block cursor parked at the top left is not part of the picture.
export const HIDE_CURSOR = "\x1b[?25l";
export const SHOW_CURSOR = "\x1b[?25h";
