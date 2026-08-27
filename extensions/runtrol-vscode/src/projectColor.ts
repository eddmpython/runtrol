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
/// # Why one slot and three names
///
/// The two surfaces cannot be handed the same thing. A terminal tab takes a `ThemeColor`, and `terminal.ansi` is
/// the family VS Code names for tab icons. The sidebar is a page under a strict CSP, where the colour has to be a
/// class its own stylesheet paints, because a nonce covers a `<style>` block and never an inline `style`
/// attribute: measured on the operator machine 2026-08-28, the band was laid out at full width and painted
/// nothing at all, for exactly that reason. So the project owns a slot and each surface names that slot the way it
/// can read it, from one list, which is what lets a tab and its heading be recognised as the same project.
///
/// # Why these five
///
/// They are theme colours, not hex: the editor is the only thing that knows what is readable in the reader's
/// theme, and a fixed hex that looks right in dark is invisible in light. Five is what both vocabularies name the
/// same way, and it is within what a person can still tell apart at icon size; a sixth would have to be cyan on
/// one surface and orange on the other, which is worse than one fewer colour because the pairing is the point.
/// Projects past the fifth reuse the ring, so the colour narrows the guess rather than settling it, and the
/// tooltip always carries the project's name for anyone who cannot use the colour at all.

/// The hues, in the order projects meet them. One row per slot, so the tab and the band can never drift apart.
///
/// `band` is a class name rather than a colour: the sidebar's page is served under a CSP that allows styles only
/// from its nonced stylesheet, and a nonce does not cover inline `style` attributes. Painting the band from an
/// attribute is what left it invisible (measured 2026-08-28), so the stylesheet carries the colour and the row
/// carries the class.
export const HUES = [
  { band: "hueBlue", tab: "terminal.ansiBlue", chart: "charts.blue" },
  { band: "hueGreen", tab: "terminal.ansiGreen", chart: "charts.green" },
  { band: "huePurple", tab: "terminal.ansiMagenta", chart: "charts.purple" },
  { band: "hueYellow", tab: "terminal.ansiYellow", chart: "charts.yellow" },
  { band: "hueRed", tab: "terminal.ansiRed", chart: "charts.red" },
] as const;

/// The slot one project holds, from the project's own workspace path.
///
/// Deterministic and free of state: the same folder is the same slot in every window and after every restart,
/// with nothing stored and nothing to migrate. `null` for a conversation with no project, which is drawn in the
/// ordinary foreground because there is no project for the colour to stand for.
function projectColorSlot(workspace: string | null): number | null {
  const key = workspace?.trim().toLowerCase() ?? "";
  if (!key) return null;
  // FNV-1a. Small, stable across runs, and spread well enough that two projects a person added together do not
  // land on the same colour. Nothing here is security-bearing.
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index += 1) {
    hash ^= key.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % HUES.length;
}

/// The theme colour id for this project's terminal tab.
export function tabColorId(workspace: string | null): string | null {
  const slot = projectColorSlot(workspace);
  return slot === null ? null : HUES[slot]?.tab ?? null;
}

/// The class this project's band wears in the sidebar, which the page's own stylesheet paints.
export function rowHueClass(workspace: string | null): string | null {
  const slot = projectColorSlot(workspace);
  return slot === null ? null : HUES[slot]?.band ?? null;
}
