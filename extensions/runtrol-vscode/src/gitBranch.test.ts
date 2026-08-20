import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { branchOfHead, readGitBranch } from "./gitBranch";

const ROOT = process.platform === "win32" ? "C:\\work" : "/work";
const REPO = path.join(ROOT, "alpha");

/// A fake filesystem: the files that exist and what they say; everything else is ENOENT.
function files(entries: Record<string, string>): (file: string) => Promise<string> {
  const known = new Map(Object.entries(entries).map(([file, text]) => [path.resolve(file), text]));
  return (file) => {
    const text = known.get(path.resolve(file));
    return text === undefined
      ? Promise.reject(Object.assign(new Error("ENOENT"), { code: "ENOENT" }))
      : Promise.resolve(text);
  };
}

test("a symbolic HEAD reads as its branch name", () => {
  assert.equal(branchOfHead("ref: refs/heads/main\n"), "main");
  assert.equal(branchOfHead("ref: refs/heads/feature/nested-name"), "feature/nested-name");
});

test("a detached HEAD reads as its short hash, which is what git status says", () => {
  assert.equal(branchOfHead("0123456789abcdef0123456789abcdef01234567\n"), "0123456");
});

test("an unreadable HEAD is no branch, never a guess", () => {
  assert.equal(branchOfHead(""), null);
  assert.equal(branchOfHead("ref: refs/remotes/origin/main"), null);
  assert.equal(branchOfHead("garbage"), null);
});

test("the branch comes from the repository the folder sits in, subfolders included", async () => {
  const read = files({ [path.join(REPO, ".git", "HEAD")]: "ref: refs/heads/main\n" });
  assert.equal(await readGitBranch(REPO, read), "main");
  assert.equal(await readGitBranch(path.join(REPO, "packages", "core"), read), "main");
});

test("a worktree's pointer file is followed to where its HEAD lives", async () => {
  const worktree = path.join(ROOT, "alpha-wt");
  const gitdir = path.join(REPO, ".git", "worktrees", "alpha-wt");
  const read = files({
    [path.join(worktree, ".git")]: `gitdir: ${gitdir}\n`,
    [path.join(gitdir, "HEAD")]: "ref: refs/heads/experiment\n",
  });
  assert.equal(await readGitBranch(worktree, read), "experiment");
});

test("a folder outside any repository has no branch", async () => {
  assert.equal(await readGitBranch(path.join(ROOT, "plain"), files({})), null);
});
