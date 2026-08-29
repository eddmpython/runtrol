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

/// Both counts, or null when the folder is not a repository, has no commit yet, or git is not on this machine.
///
/// A subprocess, unlike the branch chip, because line counts are not in any file git keeps. Two calls in
/// parallel: `diff --shortstat` for the lines, `status --porcelain=v2 --branch` for untracked files and the
/// upstream distance. `GIT_OPTIONAL_LOCKS=0` keeps `status` from refreshing the index on disk, so reading
/// never writes into a repository an agent is working in.
export async function readGitChanges(
  workspace: string,
  run: GitRunner = runGit,
): Promise<GitChanges | null> {
  try {
    const [shortstat, status] = await Promise.all([
      run(workspace, ["diff", "--shortstat", "HEAD"]),
      run(workspace, ["status", "--porcelain=v2", "--branch"]),
    ]);
    return { ...parseShortstat(shortstat), ...parseStatusBranch(status) };
  } catch {
    // Not a repository, an unborn branch, or no git at all: each means "nothing to show", never a number. The
    // branch chip beside this one already says whether the folder is in a repository.
    return null;
  }
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
      (error, stdout) => {
        if (error) reject(error);
        else resolve(stdout);
      },
    );
  });
}

/// How long a project stays quiet after its last write before it is measured again.
///
/// An agent writes to the screen in bursts and edits files between them. Measuring on every write would run git
/// hundreds of times a minute; measuring once the burst settles catches the edit it made.
const SETTLE_MS = 1_500;
/// A burst that never settles is still measured this often, so a long turn is not a blank chip until it ends.
const SETTLE_MAX_WAIT_MS = 8_000;

/// The last answer per project folder, and when to ask again.
///
/// No polling. A project is measured when it first appears, when a conversation in it writes to its screen
/// (the moment an agent can have changed files), and when the editor's own git extension reports a change in a
/// folder this window has open (a person committing by hand). A project nobody touches costs nothing.
export class GitChangesWatch {
  private readonly cache = new Map<string, GitChanges | null>();
  /// The folder as the list spells it, per key, so git is run on the path the person sees.
  private readonly folders = new Map<string, string>();
  private readonly pending = new Map<string, { timer: NodeJS.Timeout; since: number }>();
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly read: (workspace: string) => Promise<GitChanges | null> = readGitChanges,
    private readonly settleMs = SETTLE_MS,
    private readonly maxWaitMs = SETTLE_MAX_WAIT_MS,
  ) {}

  /// The last answer for this folder, or undefined when it has never been measured.
  get(workspace: string): GitChanges | null | undefined {
    return this.cache.get(keyOf(workspace));
  }

  /// Measure a folder that has never been measured. Nothing happens for one that has.
  ensure(workspace: string): void {
    const key = keyOf(workspace);
    if (this.cache.has(key) || this.pending.has(key)) return;
    this.cache.set(key, null);
    this.folders.set(key, workspace);
    void this.measure(key, workspace);
  }

  /// Something may have changed in this folder: measure once it settles.
  touch(workspace: string): void {
    if (this.disposed) return;
    const key = keyOf(workspace);
    this.folders.set(key, workspace);
    const now = Date.now();
    const waiting = this.pending.get(key);
    if (waiting) {
      clearTimeout(waiting.timer);
      if (now - waiting.since >= this.maxWaitMs) {
        this.pending.delete(key);
        void this.measure(key, workspace);
        return;
      }
    }
    const since = waiting?.since ?? now;
    const timer = setTimeout(() => {
      this.pending.delete(key);
      void this.measure(key, workspace);
    }, this.settleMs);
    this.pending.set(key, { timer, since });
  }

  /// Something changed somewhere under this root: touch every folder measured under it.
  touchUnder(root: string): void {
    const prefix = keyOf(root);
    for (const [key, folder] of this.folders) {
      if (key === prefix || key.startsWith(`${prefix}${path.sep}`)) this.touch(folder);
    }
  }

  /// Forget folders the list no longer shows.
  keep(workspaces: readonly string[]): void {
    const wanted = new Set(workspaces.map(keyOf));
    for (const key of [...this.cache.keys()]) {
      if (wanted.has(key)) continue;
      this.cache.delete(key);
      this.folders.delete(key);
    }
  }

  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  dispose(): void {
    this.disposed = true;
    for (const waiting of this.pending.values()) clearTimeout(waiting.timer);
    this.pending.clear();
    this.listeners.clear();
  }

  private async measure(key: string, workspace: string): Promise<void> {
    const next = await this.read(workspace);
    if (this.disposed) return;
    const previous = this.cache.get(key);
    this.cache.set(key, next);
    if (previous !== undefined && sameChanges(previous, next)) return;
    for (const listener of this.listeners) listener();
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
