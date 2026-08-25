import { record, type UnknownRecord } from "./presentation";

/// The most of one declared change that travels to the editor. A whole file's replacement fits; the page
/// never draws it, VS Code's own diff editor does, so the bound is the editor's comfort rather than a
/// transcript's. A change past it is cut with an ellipsis line, said on the last line.
export const MAX_DIFF_CHARACTERS = 256 * 1024;

/// The longest path a declared change may name.
const MAX_DIFF_PATH = 4096;
/// A call that touches more files than this shows the first ones; the rest stay in the raw detail.
const MAX_DIFFS = 8;

/// A change a service declared as a change, in one of the two shapes actually measured here.
///
/// - `oldNew` is the Agent Client Protocol's `content[]` block `{type: "diff", path?, oldText?, newText?}`.
/// - `unified` is the codex shape `changes[] = {path?, diff: "<unified text>"}`, at the top of a
///   `patchUpdated` body and inside `item` on an `item/started` or `item/completed` file change (measured
///   in the real window: the started frame carries `item.changes`, and a page reading only the top level
///   showed no change for a file the service had just written).
///
/// Nothing else becomes a diff. A tool argument that merely looks like a patch (Claude Code's
/// `old_string`/`new_string` input) is an argument, and rendering it as a declared change would be
/// this product claiming something the service never said.
export type DeclaredDiff =
  | { kind: "oldNew"; path: string; oldText: string; newText: string }
  | { kind: "unified"; path: string; text: string };

export type DeclaredDiffFinding = {
  diffs: DeclaredDiff[];
  /// Payload keys whose every entry was rendered as a diff. The raw detail may skip these because
  /// the same bytes are already on screen in diff form; a key with any unrendered entry is not here.
  consumed: Set<string>;
};

export function declaredDiffs(body: UnknownRecord): DeclaredDiffFinding {
  const finding: DeclaredDiffFinding = { diffs: [], consumed: new Set() };
  const payload = record(body.payload);
  if (!payload) return finding;
  harvest(payload.content, finding, "content", oldNewOf);
  harvest(payload.changes, finding, "changes", unifiedOf);
  const item = record(payload.item);
  if (item) harvest(item.changes, finding, "item", unifiedOf);
  return finding;
}

function harvest(
  value: unknown,
  finding: DeclaredDiffFinding,
  key: string,
  read: (entry: UnknownRecord) => DeclaredDiff | null,
): void {
  if (!Array.isArray(value) || value.length === 0) return;
  let every = true;
  for (const raw of value) {
    const entry = record(raw);
    const diff = entry ? read(entry) : null;
    if (!diff) {
      every = false;
      continue;
    }
    if (finding.diffs.length < MAX_DIFFS) {
      finding.diffs.push(diff);
    } else {
      every = false;
    }
  }
  if (every) finding.consumed.add(key);
}

function oldNewOf(entry: UnknownRecord): DeclaredDiff | null {
  if (entry.type !== "diff") return null;
  const oldText = typeof entry.oldText === "string" ? entry.oldText : null;
  const newText = typeof entry.newText === "string" ? entry.newText : null;
  if (oldText === null && newText === null) return null;
  return {
    kind: "oldNew",
    path: typeof entry.path === "string" ? entry.path : "",
    oldText: bounded(oldText ?? ""),
    newText: bounded(newText ?? ""),
  };
}

function unifiedOf(entry: UnknownRecord): DeclaredDiff | null {
  if (typeof entry.diff !== "string" || !entry.diff) return null;
  return {
    kind: "unified",
    path: typeof entry.path === "string" ? entry.path : "",
    text: bounded(entry.diff),
  };
}

/// Whether a value crossing the webview boundary is a declared change this host will open. Bounded like
/// everything else that crosses: the page is display code, and a hostile page must not be able to push
/// bulk into the editor through this.
export function isDeclaredDiff(value: unknown): value is DeclaredDiff {
  const candidate = record(value);
  if (!candidate) return false;
  if (typeof candidate.path !== "string" || candidate.path.length > MAX_DIFF_PATH) return false;
  const text = (field: unknown): field is string => (
    typeof field === "string" && field.length <= MAX_DIFF_CHARACTERS + 4
  );
  if (candidate.kind === "oldNew") return text(candidate.oldText) && text(candidate.newText);
  if (candidate.kind === "unified") return text(candidate.text);
  return false;
}

function bounded(text: string): string {
  return text.length > MAX_DIFF_CHARACTERS ? `${text.slice(0, MAX_DIFF_CHARACTERS)}\n...` : text;
}
