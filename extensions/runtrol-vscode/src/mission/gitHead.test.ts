import assert from "node:assert/strict";
import test from "node:test";

import { branchFromHead, gitdirTarget } from "./gitHead";

test("a symbolic HEAD names its branch and a detached HEAD names nothing", () => {
  assert.equal(branchFromHead("ref: refs/heads/main\n"), "main");
  assert.equal(branchFromHead("ref: refs/heads/feature/deep-name\n"), "feature/deep-name");
  assert.equal(branchFromHead("9d2f1c0a4b3e8d7f6a5c4b3a2d1e0f9a8b7c6d5e\n"), null, "detached");
  assert.equal(branchFromHead(""), null);
});

test("a worktree's .git file yields its gitdir target and garbage yields nothing", () => {
  assert.equal(
    gitdirTarget("gitdir: C:/repo/.git/worktrees/sprout\n"),
    "C:/repo/.git/worktrees/sprout",
  );
  assert.equal(gitdirTarget("gitdir: ../relative/.git/worktrees/x"), "../relative/.git/worktrees/x");
  assert.equal(gitdirTarget("not a pointer"), null);
  assert.equal(gitdirTarget("gitdir:   "), null);
});
