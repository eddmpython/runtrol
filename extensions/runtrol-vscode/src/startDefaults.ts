/// What the operator last chose for a project in the explicit start flow, remembered per project.
///
/// Remembered to pre-highlight, never to skip: the quick path keeps sending nothing (the installed
/// CLI's own settings stay the only automatic authority; see the null-null comment in
/// `Controller.startSession`), and the configured flow still asks every question. These values only
/// decide which row each picker opens on, so the second configured start in the same project is
/// Enter-Enter-Enter instead of scroll-and-hunt.
///
/// Stored in globalState as one bounded record keyed by workspace identity: a configuration scalar of
/// the same kind as the remembered recent service, never conversation content.

export type StartDefault = {
  providerId: string;
  model: string | null;
  effort: string | null;
  permission: string | null;
  /// When this was last used, for pruning only.
  atMs: number;
};

export type StartDefaults = Record<string, StartDefault>;

export const START_DEFAULTS_KEY = "runtrol.projectStartDefaults";

/// Bounded like every store this extension keeps: old projects fall off, they do not accumulate.
const MAX_ENTRIES = 64;

/// Read whatever a previous version stored, keeping only rows that still parse.
///
/// Defensive by field, not by version: a malformed row disappears instead of poisoning the rest,
/// which is the same posture `projects.ts` takes for its records.
export function readStartDefaults(value: unknown): StartDefaults {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return {};
  const defaults: StartDefaults = {};
  for (const [key, raw] of Object.entries(value)) {
    if (raw === null || typeof raw !== "object" || Array.isArray(raw)) continue;
    const row = raw as Record<string, unknown>;
    if (typeof row.providerId !== "string" || !row.providerId) continue;
    if (typeof row.atMs !== "number" || !Number.isFinite(row.atMs)) continue;
    defaults[key] = {
      providerId: row.providerId,
      model: typeof row.model === "string" && row.model ? row.model : null,
      effort: typeof row.effort === "string" && row.effort ? row.effort : null,
      permission: typeof row.permission === "string" && row.permission ? row.permission : null,
      atMs: row.atMs,
    };
  }
  return defaults;
}

/// Remember one project's latest explicit choice, pruning the least recently used past the bound.
export function rememberStartDefault(
  defaults: StartDefaults,
  key: string,
  choice: Omit<StartDefault, "atMs">,
  atMs: number,
): StartDefaults {
  const next: StartDefaults = { ...defaults, [key]: { ...choice, atMs } };
  const keys = Object.keys(next);
  if (keys.length <= MAX_ENTRIES) return next;
  for (const oldest of keys
    .sort((left, right) => (next[left]?.atMs ?? 0) - (next[right]?.atMs ?? 0))
    .slice(0, keys.length - MAX_ENTRIES)) {
    delete next[oldest];
  }
  return next;
}
