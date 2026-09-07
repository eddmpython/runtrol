import { execFile } from "node:child_process";
import path from "node:path";

/// What a folder's repository holds that is not committed or not pushed, shown as one chip on the project row.
///
/// `added` and `removed` are lines of tracked files against HEAD (staged and unstaged together), `untracked` is
/// files git has never seen, `ahead` is commits the branch has that its upstream does not. Committing takes the
/// first three to zero and pushing takes the last to zero, which is exactly the two questions the chip answers:
/// "is there work not saved" and "is there work not shared".
export type GitChanges = {
  readonly added: number;
  readonly removed: number;
  readonly untracked: number;
  readonly ahead: number;
};

/// Run git in a folder and hand back what it printed.
export type GitRunner = (workspace: string, args: readonly string[]) => Promise<string>;

const GIT_TIMEOUT_MS = 10_000;
const GIT_OUTPUT_MAX_BYTES = 4 * 1024 * 1024;
const ERROR_DETAIL_MAX_CHARS = 512;

/// Both counts, or null when the folder is not a repository or has no commit yet. Failed reads reject.
///
/// A subprocess, unlike the branch chip, because line counts are not in any file git keeps. Two calls in
/// parallel: `diff --shortstat` for the lines, `status --porcelain=v2 --branch` for untracked files and the
/// upstream distance. `GIT_OPTIONAL_LOCKS=0` keeps `status` from refreshing the index on disk, so reading
/// never writes into a repository an agent is working in.
export async function readGitChanges(
  workspace: string,
  run: GitRunner = runGit,
): Promise<GitChanges | null> {
  const [shortstat, status] = await Promise.allSettled([
    run(workspace, ["diff", "--shortstat", "HEAD"]),
    run(workspace, ["status", "--porcelain=v2", "--branch"]),
  ]);
  if (status.status === "rejected") {
    const error = status.reason as { code?: unknown; stderr?: unknown } | null;
    if (error?.code === 128 && typeof error.stderr === "string"
      && /^fatal: not a git repository(?: \(|$)/u.test(error.stderr)) return null;
    throw status.reason;
  }
  if (shortstat.status === "rejected") {
    if (/^# branch\.oid \(initial\)\r?$/mu.test(status.value)) return null;
    throw shortstat.reason;
  }
  return { ...parseShortstat(shortstat.value), ...parseStatusBranch(status.value) };
}

/// ` 3 files changed, 120 insertions(+), 35 deletions(-)`, or an empty line when the tree is clean.
export function parseShortstat(text: string): Pick<GitChanges, "added" | "removed"> {
  const added = /(\d+) insertions?\(\+\)/u.exec(text);
  const removed = /(\d+) deletions?\(-\)/u.exec(text);
  return {
    added: added?.[1] ? Number.parseInt(added[1], 10) : 0,
    removed: removed?.[1] ? Number.parseInt(removed[1], 10) : 0,
  };
}

/// Porcelain v2 with `--branch`: a `# branch.ab +A -B` header when there is an upstream, then one line per entry
/// where `?` starts an untracked file.
export function parseStatusBranch(text: string): Pick<GitChanges, "untracked" | "ahead"> {
  let untracked = 0;
  let ahead = 0;
  for (const line of text.split(/\r?\n/u)) {
    if (line.startsWith("? ")) {
      untracked += 1;
      continue;
    }
    const upstream = /^# branch\.ab \+(\d+) -\d+$/u.exec(line);
    if (upstream?.[1]) ahead = Number.parseInt(upstream[1], 10);
  }
  return { untracked, ahead };
}

/// Whether there is anything to draw.
export function hasChanges(changes: GitChanges | null): changes is GitChanges {
  return changes !== null
    && (changes.added > 0 || changes.removed > 0 || changes.untracked > 0 || changes.ahead > 0);
}

function runGit(workspace: string, args: readonly string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      "git",
      ["-C", workspace, ...args],
      {
        timeout: GIT_TIMEOUT_MS,
        maxBuffer: GIT_OUTPUT_MAX_BYTES,
        windowsHide: true,
        env: { ...process.env, LC_ALL: "C", GIT_OPTIONAL_LOCKS: "0" },
      },
      (error, stdout, stderr) => {
        if (error) reject(Object.assign(error, { stderr }));
        else resolve(stdout);
      },
    );
  });
}

/// How long a project stays quiet after its last write before it is measured again.
///
/// An agent writes to the screen in bursts and edits files between them. Measuring on every write would run git
/// hundreds of times a minute; measuring once the burst settles catches the edit it made.
const SETTLE_MS = 3_000;
/// A burst that never settles is still measured this often, so a long turn is not a blank chip until it ends.
const SETTLE_MAX_WAIT_MS = 15_000;
/// The least time between two measurements of one folder, whatever touches it.
///
/// A measurement is two git processes, and on Windows a process is the expensive thing: eight agents writing
/// at once made the extension host spend its time spawning git, and the sidebar's own refresh p95 went from
/// tens of milliseconds to hundreds on the Windows CI runner (2026-08-29). Five seconds keeps the chip live
/// enough to read and the process count low enough not to be felt.
const MEASURE_FLOOR_MS = 5_000;

type WatchedProject = {
  readonly workspace: string;
  value: GitChanges | null;
  error: string | null;
  measuredAt: number;
  reading: boolean;
  dirty: boolean;
  pending?: { timer: NodeJS.Timeout; since: number };
};

/// The last answer per project folder, and when to ask again.
///
/// No polling. A project is measured when it first appears, when a conversation in it writes to its screen
/// (the moment an agent can have changed files), and when the editor's own git extension reports a change in a
/// folder this window has open (a person committing by hand). A project nobody touches costs nothing.
export class GitChangesWatch {
  private readonly projects = new Map<string, WatchedProject>();
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly read: (workspace: string) => Promise<GitChanges | null> = readGitChanges,
    private readonly settleMs = SETTLE_MS,
    private readonly maxWaitMs = SETTLE_MAX_WAIT_MS,
    private readonly floorMs = MEASURE_FLOOR_MS,
  ) {}

  /// The last answer for this folder, or undefined when it has never been measured.
  get(workspace: string): GitChanges | null | undefined {
    return this.projects.get(keyOf(workspace))?.value;
  }

  getError(workspace: string): string | null {
    return this.projects.get(keyOf(workspace))?.error ?? null;
  }

  /// An explicit refresh retries every visible project through its ordinary coalescing limits.
  refresh(): void {
    for (const project of this.projects.values()) this.touch(project.workspace);
  }

  /// Measure a folder that has never been measured. Nothing happens for one that has.
  ensure(workspace: string): void {
    if (this.disposed) return;
    const key = keyOf(workspace);
    if (this.projects.has(key)) return;
    const project: WatchedProject = { workspace, value: null, error: null, measuredAt: 0, reading: false, dirty: false };
    this.projects.set(key, project);
    void this.measure(key, project);
  }

  /// Something may have changed in this folder: measure once it settles.
  ///
  /// A real change signal can recover a failed read, a new repository, or its first commit. Every state uses
  /// the same floor and coalescing limits; an unavailable answer never starts its own retry poll.
  touch(workspace: string): void {
    if (this.disposed) return;
    const key = keyOf(workspace);
    const project = this.projects.get(key);
    if (!project) {
      this.ensure(workspace);
      return;
    }
    if (project.reading) {
      project.dirty = true;
      return;
    }
    const now = Date.now();
    const waiting = project.pending;
    if (waiting) clearTimeout(waiting.timer);
    const since = waiting?.since ?? now;
    const due = Math.max(project.measuredAt + this.floorMs, Math.min(now + this.settleMs, since + this.maxWaitMs));
    const timer = setTimeout(() => {
      delete project.pending;
      void this.measure(key, project);
    }, Math.max(0, due - now));
    project.pending = { timer, since };
  }

  /// Something wrote inside this folder: touch every followed folder that contains it, which is the project
  /// the conversation is filed under. A conversation running in a subfolder of a project is that project's
  /// row, so the project's chip is what moves; the subfolder itself is never measured on its own.
  touchContaining(folder: string): void {
    const key = keyOf(folder);
    for (const [known, project] of this.projects) {
      if (key === known || key.startsWith(`${known}${path.sep}`)) this.touch(project.workspace);
    }
  }

  /// Something changed somewhere under this root: touch every folder measured under it.
  touchUnder(root: string): void {
    const prefix = keyOf(root);
    for (const [key, project] of this.projects) {
      if (key === prefix || key.startsWith(`${prefix}${path.sep}`)) this.touch(project.workspace);
    }
  }

  /// Forget folders the list no longer shows.
  keep(workspaces: readonly string[]): void {
    const wanted = new Set(workspaces.map(keyOf));
    for (const [key, project] of this.projects) {
      if (wanted.has(key)) continue;
      if (project.pending) clearTimeout(project.pending.timer);
      this.projects.delete(key);
    }
  }

  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  dispose(): void {
    this.disposed = true;
    for (const project of this.projects.values()) {
      if (project.pending) clearTimeout(project.pending.timer);
    }
    this.projects.clear();
    this.listeners.clear();
  }

  private async measure(key: string, project: WatchedProject): Promise<void> {
    if (this.disposed || this.projects.get(key) !== project) return;
    project.reading = true;
    project.measuredAt = Date.now();
    let next: GitChanges | null = null;
    let error: string | null = null;
    try {
      next = await this.read(project.workspace);
    } catch (failure) {
      // A failed probe is visible and the next real change retries it through the same bounded scheduler.
      // Do not retain a stale count as though it described the current working tree.
      error = (failure instanceof Error ? failure.message : String(failure)).slice(0, ERROR_DETAIL_MAX_CHARS)
        || "Git could not read this repository";
    }
    if (this.disposed || this.projects.get(key) !== project) return;
    const changed = !sameChanges(project.value, next) || project.error !== error;
    project.value = next;
    project.error = error;
    project.reading = false;
    if (project.dirty) {
      project.dirty = false;
      this.touch(project.workspace);
    }
    if (changed) for (const listener of this.listeners) listener();
  }
}

function sameChanges(a: GitChanges | null, b: GitChanges | null): boolean {
  if (a === null || b === null) return a === b;
  return a.added === b.added && a.removed === b.removed && a.untracked === b.untracked && a.ahead === b.ahead;
}

/// One key per folder however it was spelled: the same project reaches here from a conversation's home folder
/// and from the git extension's repository root.
function keyOf(workspace: string): string {
  const resolved = path.resolve(workspace);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}
