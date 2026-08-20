import { record, type UnknownRecord } from "./presentation";

/// Same ceiling as the raw tool detail: a diff that replaces a whole file is opened in the editor,
/// not scrolled in a transcript panel.
const MAX_DIFF_CHARACTERS = 4000;
/// A call that touches more files than this shows the first ones; the rest stay in the raw detail.
const MAX_DIFFS = 8;

/// A change a service declared as a change, in one of the two shapes actually measured here.
///
/// - `oldNew` is the Agent Client Protocol's `content[]` block `{type: "diff", path?, oldText?, newText?}`.
/// - `unified` is the codex shape `changes[] = {path?, diff: "<unified text>"}`.
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

/// How one unified-diff line is coloured, read from nothing but its own first characters.
export function unifiedLineKind(line: string): "add" | "del" | "hunk" | "context" {
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  if (line.startsWith("@@")) return "hunk";
  return "context";
}

function bounded(text: string): string {
  return text.length > MAX_DIFF_CHARACTERS ? `${text.slice(0, MAX_DIFF_CHARACTERS)}\n...` : text;
}
