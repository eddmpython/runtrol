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
/// The two surfaces cannot be handed the same thing. A terminal tab takes a `ThemeColor`, and the non-bright
/// `terminal.ansi` family is the whole vocabulary VS Code paints on tab icons (its own colour stylesheet is
/// generated from exactly that list; any other id is silently unpainted). The sidebar is a page under a strict
/// CSP, where the colour has to be a class its own stylesheet paints, because a nonce covers a `<style>` block
/// and never an inline `style` attribute: measured on the operator machine 2026-08-28, the band was laid out at
/// full width and painted nothing at all, for exactly that reason. So the project owns a slot and each surface
/// names that slot the way it can read it, from one list, which is what lets a tab and its heading be recognised
/// as the same project.
///
/// # Why twelve, and why the tab repeats after six
///
/// Five slots put two of the operator's six projects in the same colour twice over (operator, 2026-08-29: the
/// palette keeps handing out the same colour; hold more). The tab side cannot grow past six: black and white are
/// not identity colours and bright variants are excluded from the editor's own tab stylesheet. So the sidebar
/// band, which this page paints itself, carries twelve hues, and each of the six extras shares its tab colour
/// with the base hue of its family. Two projects that land in one family are still told apart everywhere the
/// band is (the heading and every row), and their tabs narrow the guess to that family instead of settling it.
/// The tooltip always carries the project's name for anyone who cannot use the colour at all.
///
/// # Where the band's colours come from
///
/// The first six are the editor's own terminal palette, read as CSS variables, so the band and the tab are one
/// colour by construction in whatever theme is on. The six extras have no editor name, so they are fixed pairs:
/// one for dark themes and one for light, applied by the theme kind VS Code stamps on the page's body. A single
/// hex for both was the mistake this palette was built to avoid.

/// The hues, in the order projects meet them. One row per slot, so the tab and the band can never drift apart.
///
/// `band` is a class name rather than a colour: the stylesheet carries the colour and the row carries the class
/// (CSP, above). `dark` and `light` are what the stylesheet paints the band, per theme kind.
export const HUES = [
  { band: "hueBlue", tab: "terminal.ansiBlue", dark: "var(--vscode-terminal-ansiBlue)", light: "var(--vscode-terminal-ansiBlue)" },
  { band: "hueGreen", tab: "terminal.ansiGreen", dark: "var(--vscode-terminal-ansiGreen)", light: "var(--vscode-terminal-ansiGreen)" },
  { band: "huePurple", tab: "terminal.ansiMagenta", dark: "var(--vscode-terminal-ansiMagenta)", light: "var(--vscode-terminal-ansiMagenta)" },
  { band: "hueYellow", tab: "terminal.ansiYellow", dark: "var(--vscode-terminal-ansiYellow)", light: "var(--vscode-terminal-ansiYellow)" },
  { band: "hueRed", tab: "terminal.ansiRed", dark: "var(--vscode-terminal-ansiRed)", light: "var(--vscode-terminal-ansiRed)" },
  { band: "hueCyan", tab: "terminal.ansiCyan", dark: "var(--vscode-terminal-ansiCyan)", light: "var(--vscode-terminal-ansiCyan)" },
  // The band-only extras. Each shares its family's tab colour; the pairs are picked to stay apart from the six
  // above and from each other at band width, in dark and in light.
  { band: "hueOrange", tab: "terminal.ansiYellow", dark: "#d18616", light: "#b35900" },
  { band: "hueTeal", tab: "terminal.ansiCyan", dark: "#2bb3a8", light: "#0f766e" },
  { band: "huePink", tab: "terminal.ansiMagenta", dark: "#e879b6", light: "#be3b88" },
  { band: "hueLime", tab: "terminal.ansiGreen", dark: "#a3be3c", light: "#5f7d0e" },
  { band: "hueBrown", tab: "terminal.ansiRed", dark: "#c8a17a", light: "#8a5a2b" },
  { band: "hueSlate", tab: "terminal.ansiBlue", dark: "#8ea3b8", light: "#52606d" },
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
