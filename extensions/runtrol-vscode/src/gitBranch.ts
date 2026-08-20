import { readFile } from "node:fs/promises";
import path from "node:path";

/// The branch a folder is on, read from its own repository, or null when it is not in one.
///
/// A fact about the folder the conversation runs in, shown as a chip beside the project the way the
/// Codex app draws it. Read straight from `.git/HEAD` rather than through the editor's git extension,
/// because that extension only knows the folders this window has open and the panel shows conversations
/// from the whole machine. Bounded reads and no subprocess: `git rev-parse` was measured at 82 to 155 ms
/// per call here, and the chip is repainted on every session update.
///
/// Walks up at most `MAX_ANCESTORS` folders, because a conversation often runs in a subfolder of the
/// repository. Worktrees and submodules are handled through the `gitdir:` pointer file. A detached head
/// reads as its short hash, which is what `git status` says too.
const MAX_ANCESTORS = 8;
const MAX_POINTER_BYTES = 4_096;
const MAX_HEAD_BYTES = 4_096;

export async function readGitBranch(
  workspace: string,
  read: (file: string) => Promise<string> = readBounded,
): Promise<string | null> {
  let folder = path.resolve(workspace);
  for (let depth = 0; depth < MAX_ANCESTORS; depth += 1) {
    const gitDirectory = await resolveGitDirectory(path.join(folder, ".git"), read);
    if (gitDirectory !== null) {
      return branchOfHead(await read(path.join(gitDirectory, "HEAD")).catch(() => ""));
    }
    const parent = path.dirname(folder);
    if (parent === folder) return null;
    folder = parent;
  }
  return null;
}

/// Where this `.git` entry keeps its HEAD: the directory itself, or the one a pointer file names.
async function resolveGitDirectory(
  entry: string,
  read: (file: string) => Promise<string>,
): Promise<string | null> {
  const head = await read(path.join(entry, "HEAD")).catch(() => null);
  if (head !== null) return entry;
  const pointer = await read(entry).catch(() => null);
  if (pointer === null) return null;
  const match = /^gitdir:\s*(.+?)\s*$/mu.exec(pointer);
  if (!match?.[1]) return null;
  return path.resolve(path.dirname(entry), match[1]);
}

/// The branch name a HEAD file says, or the short hash of a detached head, or null for an unreadable one.
export function branchOfHead(head: string): string | null {
  const line = head.split(/\r?\n/u, 1)[0]?.trim() ?? "";
  if (!line) return null;
  const symbolic = /^ref:\s*refs\/heads\/(.+)$/u.exec(line);
  if (symbolic?.[1]) return symbolic[1];
  if (/^[0-9a-f]{40,64}$/iu.test(line)) return line.slice(0, 7);
  return null;
}

async function readBounded(file: string): Promise<string> {
  const bytes = await readFile(file);
  const limit = file.endsWith("HEAD") ? MAX_HEAD_BYTES : MAX_POINTER_BYTES;
  return bytes.subarray(0, limit).toString("utf8");
}
