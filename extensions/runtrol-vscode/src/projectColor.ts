/// One deterministic accent per project.
///
/// Sidebar SVG and registered terminal color glyphs derive from the same outline and palette. Their exact match
/// does not depend on two renderers resolving theme variables alike.

import path from "node:path";
import { workspaceIdentity } from "./workspaceCollision";
import { PROJECT_ACCENTS } from "./projectAccents";

export { PROJECT_ACCENTS } from "./projectAccents";

const PROJECTLESS_ACCENT = PROJECT_ACCENTS[0];

function projectColorSlot(workspace: string | null, platform: NodeJS.Platform): number | null {
  const value = workspace?.trim() ?? "";
  if (!value) return null;
  const key = workspaceIdentity(value, platform === "win32" ? path.win32 : path.posix, platform);
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index += 1) {
    hash ^= key.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % PROJECT_ACCENTS.length;
}

/// Exact colour embedded into both provider glyphs while a conversation tab is open. Projectless conversations use
/// one stable fallback so their two surfaces still match.
export function projectAccentColor(workspace: string | null, platform: NodeJS.Platform = process.platform): string {
  const slot = projectColorSlot(workspace, platform);
  return slot === null ? PROJECTLESS_ACCENT : PROJECT_ACCENTS[slot] ?? PROJECTLESS_ACCENT;
}

/// Keep an existing project's accent when other projects are added or reordered. Prefer its path-derived colour,
/// then a free palette entry; only a full palette reuses a colour. The project record persists the selection.
export function availableProjectAccent(workspace: string, used: ReadonlySet<string>): string {
  const preferred = projectAccentColor(workspace);
  if (!used.has(preferred)) return preferred;
  return PROJECT_ACCENTS.find((accent) => !used.has(accent)) ?? preferred;
}

export function isProjectAccent(value: unknown): value is string {
  return typeof value === "string" && PROJECT_ACCENTS.some((accent) => accent === value);
}
