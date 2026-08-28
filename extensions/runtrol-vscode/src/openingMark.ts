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
const TURNS: ReadonlyArray<readonly [string, string, string, string]> = [
  ["╭", "╮", "╯", "╰"],
  ["╰", "╭", "╮", "╯"],
  ["╯", "╰", "╭", "╮"],
  ["╮", "╯", "╰", "╭"],
];

const BLOCK_ROWS = 3;
const BLOCK_COLUMNS = 5;

/// One quarter turn of the mark as the lines it draws, top to bottom.
export function markFrame(at: number): string[] {
  const corners = TURNS[at % TURNS.length];
  if (!corners) return [];
  const [topLeft, topRight, bottomRight, bottomLeft] = corners;
  return [
    `${topLeft}   ${topRight}`,
    "     ",
    `${bottomLeft}   ${bottomRight}`,
  ];
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

/// How often a frame is drawn. Four quarter turns at this interval is one turn every two thirds of a second,
/// which reads as motion without asking the terminal to repaint faster than it can.
export const MARK_FRAME_MS = 160;

/// Hide the cursor while the mark turns; a block cursor parked at the top left is not part of the picture.
export const HIDE_CURSOR = "\x1b[?25l";
export const SHOW_CURSOR = "\x1b[?25h";
