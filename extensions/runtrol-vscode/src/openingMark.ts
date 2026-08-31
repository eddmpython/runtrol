/// The Runtrol mark, standing in the middle of a conversation tab while it opens, with a coral light passing
/// over it.
///
/// # Why anything is drawn at all
///
/// Opening a conversation asks the Runtime for a terminal, which starts or attaches to a coding CLI. That is
/// fast but it is not instant, and until the CLI writes its first byte the tab is an empty black rectangle.
/// An empty rectangle says the same thing as a broken one, so the person waits without knowing whether they
/// are waiting (operator, 2026-08-28: show the symbol moving, like a spinner).
///
/// # Why it is the mark itself and not characters that resemble it
///
/// A terminal has no drawing surface, only cells. The first attempts put four rounded box elbows in five cells
/// and called them the four arms; on screen they were anyone's brackets, and the operator said so, several
/// times over (2026-08-28 to 2026-08-30: use our symbol, as it is). A cell is not only a character, though.
/// The half blocks `▀` `▄` `█` split it into an upper and a lower pixel, each in its own colour, so a block of
/// them is a raster with cells for pixels, and a raster can be the mark. The pixels come from the brand's own
/// renderer: `assets/brand/render.py` draws the hinted 16 px geometry (the favicon's, whose stroke, gap and
/// radius land on whole cells) as `symbol-cells.json`, and this module paints that grid and nothing else. No
/// geometry lives here.
///
/// The mark does not spin (operator, 2026-08-29: use our symbol as it is and put the effect on it, not a
/// rotation). The light is what moves; the mark is the thing the light moves across.

import symbolCells from "../../../assets/brand/symbol-cells.json";

type Arm = "accent" | "ink";
type Pixel = Arm | null;

/// One character cell of the block: its glyph and the arm it belongs to. A cell is one colour by construction
/// (see `pairRows`), so one arm is enough to style it.
interface Cell {
  readonly glyph: string;
  readonly arm: Pixel;
}

/// The grid as the brand renderer wrote it, one pixel per letter.
function readPixels(): Pixel[][] {
  const { accent, ink, empty, rows } = symbolCells;
  const width = rows[0]?.length ?? 0;
  return rows.map((row, y) => {
    if (row.length !== width) throw new Error(`symbol-cells.json row ${y} is ${row.length} wide, not ${width}`);
    return [...row].map((letter, x): Pixel => {
      if (letter === accent) return "accent";
      if (letter === ink) return "ink";
      if (letter === empty) return null;
      throw new Error(`symbol-cells.json row ${y} column ${x} holds ${JSON.stringify(letter)}, not a legend letter`);
    });
  });
}

/// Two pixel rows become one row of half-block cells: the upper pixel is `▀`, the lower `▄`, both `█`.
///
/// A cell carries a single foreground colour, so its two pixels must agree. They do: the four arms meet only
/// across the centre gap, which is four pixel rows tall and starts on an even row, so no cell straddles a
/// coral pixel and an ink one. The grid is generated and this shape is tested; the throws below are the
/// invariant the painter relies on, not a condition the product meets at run time.
function pairRows(pixels: readonly (readonly Pixel[])[]): Cell[][] {
  if (pixels.length % 2 !== 0) throw new Error(`symbol-cells.json has ${pixels.length} rows; half blocks need an even count`);
  const cells: Cell[][] = [];
  for (let y = 0; y < pixels.length; y += 2) {
    const upper = pixels[y] ?? [];
    const lower = pixels[y + 1] ?? [];
    cells.push(
      upper.map((top, x): Cell => {
        const bottom = lower[x] ?? null;
        if (top !== null && bottom !== null && top !== bottom) {
          throw new Error(`symbol-cells.json rows ${y} and ${y + 1} column ${x} put two colours in one cell`);
        }
        if (top !== null && bottom !== null) return { glyph: "█", arm: top };
        if (top !== null) return { glyph: "▀", arm: top };
        if (bottom !== null) return { glyph: "▄", arm: bottom };
        return { glyph: " ", arm: null };
      }),
    );
  }
  return cells;
}

const PIXELS: readonly (readonly Pixel[])[] = readPixels();
const CELLS: readonly (readonly Cell[])[] = pairRows(PIXELS);
const BLOCK_ROWS = CELLS.length;
const BLOCK_COLUMNS = CELLS[0]?.length ?? 0;

/// The mark as the pixel grid the brand renderer wrote, for a test to read its shape without the escapes.
export function markPixels(): readonly (readonly Pixel[])[] {
  return PIXELS;
}

/// The Runtrol coral as a truecolor foreground, and the light: the same coral lifted halfway to white at its
/// core and a quarter of the way at its edge. A light of our colour, so the motion reads as our light rather
/// than as the terminal blinking. Away from the light the arms stand in their own colours at full strength;
/// nothing is dimmed, because the mark at rest is the brand and not a shadow of it.
const CORAL = "38;2;245;101;101";
const CORAL_LIT = "38;2;250;178;178";
const CORAL_EDGE = "38;2;248;140;140";
/// The ink arms take the terminal's own foreground, so the mark's ink follows the surface the way the brand
/// asks: graphite on a light theme, white on a dark one.
const INK = "39";

/// The light is a vertical band: a core three cells wide with a softer cell on either side. It moves two
/// cells a frame. The sweep starts and ends well outside the block, so the light fully leaves on the right
/// and the mark stands unlit for a few frames before the light enters again on the left: a repeating
/// left-to-right pass with a rest between passes, not a band that jumps straight back to the start.
const LIGHT_CORE = 1;
const LIGHT_EDGE = 2;
const LIGHT_STEP = 2;
const REST_FRAMES = 3;
const SWEEP_MARGIN = LIGHT_EDGE + 1 + LIGHT_STEP * REST_FRAMES;
const SWEEP_PERIOD = BLOCK_COLUMNS + SWEEP_MARGIN * 2;

/// The SGR parameters for one cell at column `x` of an arm, given where the light is (`lit`).
function cellStyle(arm: Arm, x: number, lit: number): string {
  const distance = Math.abs(x - lit);
  if (distance <= LIGHT_CORE) return CORAL_LIT;
  if (distance <= LIGHT_EDGE) return CORAL_EDGE;
  return arm === "accent" ? CORAL : INK;
}

/// The escape sequence that paints one frame centred in a terminal of this size.
///
/// The mark stands still and the coral light sweeps across it left to right, over and over. The whole screen
/// is cleared each frame rather than only the block: the terminal this draws into is about to be handed to a
/// CLI that will clear it anyway, and a partial repaint of a pane that may have been resized leaves the old
/// block behind. Within a row the style is only re-sent where it changes, and empty cells are plain spaces.
///
/// A pane smaller than the block gets the part of the block that fits, from its top left. Writing past the
/// right edge would wrap, and a wrap on the last row scrolls, so a pane narrower than the mark would grow
/// scrollback by a line every frame; a pane shorter than it would pile the lower rows onto the last one.
export function paintMark(at: number, columns: number, rows: number): string {
  // Cursor positions are one-based: the rows above the block and below it are equal for an even pane.
  const top = Math.max(0, Math.floor((rows - BLOCK_ROWS) / 2)) + 1;
  const left = Math.max(0, Math.floor((columns - BLOCK_COLUMNS) / 2)) + 1;
  const visibleRows = Math.min(BLOCK_ROWS, Math.max(0, rows - top + 1));
  const visibleColumns = Math.min(BLOCK_COLUMNS, Math.max(0, columns - left + 1));
  const lit = ((at * LIGHT_STEP) % SWEEP_PERIOD) - SWEEP_MARGIN;
  const painted = CELLS.slice(0, visibleRows).map((row, y) => {
    let style: string | null = null;
    let line = `\x1b[${top + y};${left}H`;
    row.slice(0, visibleColumns).forEach((cell, x) => {
      if (cell.arm === null) {
        line += cell.glyph;
        return;
      }
      const wanted = cellStyle(cell.arm, x, lit);
      if (wanted !== style) {
        line += `\x1b[0;${wanted}m`;
        style = wanted;
      }
      line += cell.glyph;
    });
    return `${line}\x1b[0m`;
  }).join("");
  return `\x1b[2J${painted}`;
}

/// How often a frame is drawn. The light moves two cells a frame, and at this interval a pass across the
/// mark takes about a second and a half, which reads as motion without asking the terminal to repaint faster
/// than it can.
export const MARK_FRAME_MS = 70;

/// Hide the cursor while the mark stands; a block cursor parked at the top left is not part of the picture.
export const HIDE_CURSOR = "\x1b[?25l";
export const SHOW_CURSOR = "\x1b[?25h";

/// Whether a chunk the service wrote puts anything on the screen a person could see.
///
/// Escape sequences move the cursor, clear the screen and set colours without drawing a mark, and a screen
/// handed back for a conversation that has not started is exactly that. The scan is deliberately shallow: it
/// steps over the escape sequences and asks whether any non-blank character is left. It never looks at what
/// the characters say.
///
/// The stepping follows the shape of the sequences themselves. `ESC [` opens a control sequence that runs to
/// its final byte (`@` to `~`); `ESC ]`, `ESC P`, `ESC X`, `ESC ^` and `ESC _` open a string that runs to a
/// BEL or to `ESC \`; any other `ESC` is a short sequence that ends at its first byte outside the
/// intermediates. Treating the `[` itself as a final byte, as an earlier version did, counted `ESC [ 2 J` as
/// visible text and took the mark down on the first clear-screen a service sent.
export function hasVisibleText(text: string): boolean {
  type State = "plain" | "escape" | "control" | "string";
  let state: State = "plain";
  for (const character of text) {
    switch (state) {
      case "plain":
        if (character === "\x1b") state = "escape";
        else if (character > " ") return true;
        break;
      case "escape":
        if (character === "[") state = "control";
        else if ("]PX^_".includes(character)) state = "string";
        else if (character < " " || character > "/") state = "plain";
        break;
      case "control":
        if (character >= "@" && character <= "~") state = "plain";
        break;
      case "string":
        if (character === "\x07") state = "plain";
        // `ESC \` ends the string; the backslash is then the short sequence's final byte.
        else if (character === "\x1b") state = "escape";
        break;
    }
  }
  return false;
}
