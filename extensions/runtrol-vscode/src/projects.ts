import * as path from "node:path";

import { workspaceIdentity } from "./workspaceCollision";

/// One project the operator created, by hand, in the panel.
///
/// # A project is a decision, not a discovery
///
/// The panel used to invent a heading for every folder that happened to hold a conversation. On a machine full
/// of conversations that is a wall of folder names nobody asked for, and the operator rejected it in exactly
/// those words (2026-08-19: thirty auto-headings named workspace-1 through workspace-30). The chat apps people
/// already use got this right: a project exists because somebody made it, conversations are filed into it by
/// where they happen, and a conversation nobody filed is simply a conversation.
export type ProjectRecord = {
  /// The folder's identity, which is what conversations are matched against.
  readonly key: string;
  /// What the person calls it. Defaults to the folder's name and stays whatever they rename it to.
  readonly name: string;
  /// The folder, first spelling kept.
  readonly workspace: string;
  /// Whether the person pinned it to the top of the list. A placement choice, never a fact about the folder.
  readonly pinned: boolean;
};

/// The slice of a `vscode.Memento` this store needs, named so tests can hand in a plain object.
export type ProjectMemento = {
  get(key: string): unknown;
  update(key: string, value: unknown): Thenable<void>;
};

const STORAGE_KEY = "runtrol.projects";

/// The operator's projects, persisted across windows and sessions.
///
/// Global rather than per-workspace state, because the panel is one management surface for the whole machine:
/// a project created in one window is the same project in every other.
export class ProjectStore {
  private records: ProjectRecord[];
  private readonly listeners = new Set<() => void>();

  constructor(private readonly memento: ProjectMemento) {
    this.records = readRecords(this.memento.get(STORAGE_KEY));
  }

  all(): readonly ProjectRecord[] {
    return this.records;
  }

  /// Announce every change once, after it is persisted. The tree redraws from `all()`.
  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  /// Create a project on a folder. Creating the same folder twice is the same project, not an error.
  async create(workspace: string, name?: string): Promise<ProjectRecord> {
    const key = workspaceIdentity(workspace);
    const existing = this.records.find((record) => record.key === key);
    if (existing) return existing;
    const record: ProjectRecord = {
      key,
      name: (name ?? path.basename(workspace)).trim() || path.basename(workspace) || workspace,
      workspace,
      pinned: false,
    };
    await this.replace([...this.records, record]);
    return record;
  }

  /// Set what the person calls the project on this folder. A blank name is refused rather than silently kept.
  async setName(workspace: string, name: string): Promise<void> {
    const trimmed = name.trim();
    if (!trimmed) {
      throw new Error("a project needs a name");
    }
    const key = workspaceIdentity(workspace);
    const next = this.records.map((record) => (
      record.key === key ? { ...record, name: trimmed } : record
    ));
    await this.replace(next);
  }

  /// Pin or unpin the project on this folder. Pinned projects sit first, in the order they were added.
  async setPinned(workspace: string, pinned: boolean): Promise<void> {
    const key = workspaceIdentity(workspace);
    const next = this.records.map((record) => (
      record.key === key ? { ...record, pinned } : record
    ));
    await this.replace(next);
  }

  /// Remove the project on this folder. Its conversations lose their heading and nothing else: removal files
  /// nothing, deletes nothing, and adding the project again is one click. The folder on disk is never
  /// touched: removing a project is a list decision, deleting a folder is not one this surface makes.
  async remove(workspace: string): Promise<void> {
    const key = workspaceIdentity(workspace);
    await this.replace(this.records.filter((record) => record.key !== key));
  }

  private async replace(next: ProjectRecord[]): Promise<void> {
    await this.memento.update(STORAGE_KEY, next.map((record) => ({
      name: record.name,
      workspace: record.workspace,
      pinned: record.pinned,
    })));
    this.records = next;
    for (const listener of this.listeners) listener();
  }
}

/// Read whatever a past version persisted, keeping every entry that still makes sense and dropping the rest.
///
/// The key is recomputed rather than trusted from disk, so a change to the identity function can never strand
/// a record under an identity nothing else computes any more.
function readRecords(raw: unknown): ProjectRecord[] {
  if (!Array.isArray(raw)) return [];
  const records: ProjectRecord[] = [];
  const seen = new Set<string>();
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const { name, workspace, pinned } = entry as { name?: unknown; workspace?: unknown; pinned?: unknown };
    if (typeof name !== "string" || typeof workspace !== "string") continue;
    if (!name.trim() || !workspace.trim()) continue;
    const key = workspaceIdentity(workspace);
    if (seen.has(key)) continue;
    seen.add(key);
    records.push({ key, name, workspace, pinned: pinned === true });
  }
  return records;
}
