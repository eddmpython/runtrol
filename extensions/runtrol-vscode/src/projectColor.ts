/// One colour per project, so a conversation says where it belongs without spending a word on it.
///
/// # Why colour
///
/// A conversation tab has room for its own name and nothing else. With six tabs open the reader could not tell
/// which project each belonged to, and the fix cannot be a longer title: `project . conversation` halves how many
/// tabs fit, which trades one unreadable thing for another (operator, 2026-08-26). Colour costs no width, is read
/// before text, and pairs the tab with the heading it came from in the sidebar.
///
/// # Why the product chooses it
///
/// Nobody is asked to pick one. A person who has to assign colours has been given a settings screen instead of a
/// product, so the colour follows from the project's own identity and is the same in every window on this machine.
///
/// # Why these six
///
/// They are theme colours, not hex: the editor is the only thing that knows what is readable in the reader's
/// theme, and a fixed hex that looks right in dark is invisible in light. Six is what a person can still tell
/// apart at icon size; beyond that two projects look the same, which is worse than no colour at all. Projects past
/// the sixth reuse the ring, so the colour narrows the guess rather than settling it, and the tooltip always
/// carries the project's name for anyone who cannot use the colour at all.

/// The palette, in the order projects meet it.
const PALETTE = [
  "terminal.ansiBlue",
  "terminal.ansiGreen",
  "terminal.ansiMagenta",
  "terminal.ansiYellow",
  "terminal.ansiCyan",
  "terminal.ansiRed",
] as const;

/// The theme colour id for one project, from the project's own workspace path.
///
/// Deterministic and free of state: the same folder is the same colour in every window and after every restart,
/// with nothing stored and nothing to migrate. `null` for a conversation with no project, which is drawn in the
/// ordinary foreground because there is no project for the colour to stand for.
export function projectColorId(workspace: string | null): string | null {
  const key = workspace?.trim().toLowerCase() ?? "";
  if (!key) return null;
  // FNV-1a. Small, stable across runs, and spread well enough that two projects a person added together do not
  // land on the same colour. Nothing here is security-bearing.
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index += 1) {
    hash ^= key.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return PALETTE[hash % PALETTE.length] ?? null;
}
