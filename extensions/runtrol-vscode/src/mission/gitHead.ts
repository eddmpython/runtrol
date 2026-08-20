/// Reading which branch a folder is on, from the repository's own files.
///
/// Pure parsing only, so it can be proved in plain Node; the one caller does the reading. Read-only
/// by construction: `.git/HEAD` (and the one-line `gitdir:` indirection a linked worktree uses) is
/// the repository's own statement of where it stands, and it is prefilled rather than assumed.
/// Every failure yields null and the caller falls back to asking, because a wrong branch baked into
/// a Mission base is worse than one extra keystroke.

/// The branch a HEAD file names, or null for a detached HEAD or anything unreadable.
export function branchFromHead(head: string): string | null {
  const match = /^ref:\s+refs\/heads\/(\S+)/u.exec(head.trim());
  return match?.[1] ?? null;
}

/// The target a `.git` file points at (a linked worktree's real git directory), or null.
export function gitdirTarget(dotGitFile: string): string | null {
  const match = /^gitdir:\s*(.+)$/u.exec(dotGitFile.trim());
  const target = match?.[1]?.trim();
  return target || null;
}
