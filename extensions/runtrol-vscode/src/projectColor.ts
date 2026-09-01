/// One deterministic accent per project.
///
/// The sidebar and editor terminal both draw the provider's own SVG. VS Code does not tint a custom terminal icon
/// URI with `ThemeColor`, so both surfaces receive the same SVG with the same fixed accent embedded in it. This is
/// the exact match the operator sees, and it does not depend on two renderers resolving theme variables alike.

export const PROJECT_ACCENTS = [
  "#4e94ce",
  "#48a868",
  "#b07bd8",
  "#c69214",
  "#df5b57",
  "#36a7b8",
  "#d18616",
  "#2bb3a8",
  "#e879b6",
  "#93ae32",
  "#b88961",
  "#71869b",
] as const;

const PROJECTLESS_ACCENT = PROJECT_ACCENTS[0];

function projectColorSlot(workspace: string | null): number | null {
  const key = workspace?.trim().toLowerCase() ?? "";
  if (!key) return null;
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index += 1) {
    hash ^= key.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % PROJECT_ACCENTS.length;
}

/// Exact colour embedded into both provider glyphs while a conversation tab is open. Projectless conversations use
/// one stable fallback so their two surfaces still match.
export function projectAccentColor(workspace: string | null): string {
  const slot = projectColorSlot(workspace);
  return slot === null ? PROJECTLESS_ACCENT : PROJECT_ACCENTS[slot] ?? PROJECTLESS_ACCENT;
}
