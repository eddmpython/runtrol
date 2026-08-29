import assert from "node:assert/strict";
import test from "node:test";

import {
  GitChangesWatch,
  hasChanges,
  parseShortstat,
  parseStatusBranch,
  readGitChanges,
  type GitChanges,
} from "./gitChanges";

const WORKSPACE = process.platform === "win32" ? "C:\\work\\app" : "/work/app";

test("shortstat lines become added and removed counts, and a clean tree is zero", () => {
  assert.deepEqual(parseShortstat(" 3 files changed, 120 insertions(+), 35 deletions(-)\n"), { added: 120, removed: 35 });
  assert.deepEqual(parseShortstat(" 1 file changed, 1 insertion(+)\n"), { added: 1, removed: 0 });
  assert.deepEqual(parseShortstat(" 1 file changed, 1 deletion(-)\n"), { added: 0, removed: 1 });
  assert.deepEqual(parseShortstat(""), { added: 0, removed: 0 });
});

test("porcelain v2 gives the untracked files and how far ahead of upstream the branch is", () => {
  const text = [
    "# branch.oid 0123456789abcdef0123456789abcdef01234567",
    "# branch.head main",
    "# branch.upstream origin/main",
    "# branch.ab +3 -0",
    "1 .M N... 100644 100644 100644 abc def src/a.ts",
    "? notes.md",
    "? src/new.ts",
    "",
  ].join("\n");
  assert.deepEqual(parseStatusBranch(text), { untracked: 2, ahead: 3 });
});

test("a branch with no upstream is not ahead of anything", () => {
  assert.deepEqual(parseStatusBranch("# branch.head main\n"), { untracked: 0, ahead: 0 });
});

test("the two git calls are combined, and a folder git refuses is null rather than zeros", async () => {
  const answers: Record<string, string> = {
    "diff --shortstat HEAD": " 2 files changed, 10 insertions(+), 4 deletions(-)\n",
    "status --porcelain=v2 --branch": "# branch.ab +1 -0\n? x\n",
  };
  const run = (_workspace: string, args: readonly string[]) => Promise.resolve(answers[args.join(" ")] ?? "");
  assert.deepEqual(await readGitChanges(WORKSPACE, run), { added: 10, removed: 4, untracked: 1, ahead: 1 });
  const refused = () => Promise.reject(new Error("fatal: not a git repository"));
  assert.equal(await readGitChanges(WORKSPACE, refused), null);
});

test("only a non-zero count is something to draw", () => {
  assert.equal(hasChanges(null), false);
  assert.equal(hasChanges({ added: 0, removed: 0, untracked: 0, ahead: 0 }), false);
  assert.equal(hasChanges({ added: 0, removed: 0, untracked: 0, ahead: 2 }), true);
});

function counting(): { reads: string[]; read: (workspace: string) => Promise<GitChanges | null>; answer: GitChanges } {
  const state = {
    reads: [] as string[],
    answer: { added: 1, removed: 0, untracked: 0, ahead: 0 } as GitChanges,
    read: async (workspace: string): Promise<GitChanges | null> => {
      state.reads.push(workspace);
      return state.answer;
    },
  };
  return state;
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a project is measured once on first sight and again only when touched", async () => {
  const git = counting();
  const watch = new GitChangesWatch(git.read, 5, 50);
  let changed = 0;
  watch.onDidChange(() => { changed += 1; });
  assert.equal(watch.get(WORKSPACE), undefined);
  watch.ensure(WORKSPACE);
  watch.ensure(WORKSPACE);
  await tick();
  assert.equal(git.reads.length, 1);
  assert.deepEqual(watch.get(WORKSPACE), git.answer);
  assert.equal(changed, 1, "the first answer redraws");
  await tick();
  assert.equal(git.reads.length, 1, "nothing polls");
  watch.dispose();
});

test("touches inside a burst settle into one measurement, and a burst that never settles is measured anyway", async () => {
  const git = counting();
  // Generous margins: a timer on a loaded machine lands late, and a settle shorter than that jitter made this
  // test read a settled burst as a running one.
  const watch = new GitChangesWatch(git.read, 80, 250);
  watch.ensure(WORKSPACE);
  await tick();
  assert.equal(git.reads.length, 1);
  for (let index = 0; index < 5; index += 1) {
    watch.touch(WORKSPACE);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.equal(git.reads.length, 1, "still inside the burst");
  await new Promise((resolve) => setTimeout(resolve, 160));
  assert.equal(git.reads.length, 2, "the burst settled into one read");
  const started = Date.now();
  while (Date.now() - started < 400) {
    watch.touch(WORKSPACE);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.ok(git.reads.length >= 3, "a burst longer than the ceiling was measured while it ran");
  watch.dispose();
});

test("an unchanged answer does not redraw, a changed one does", async () => {
  const git = counting();
  const watch = new GitChangesWatch(git.read, 1, 10);
  let changed = 0;
  watch.onDidChange(() => { changed += 1; });
  watch.ensure(WORKSPACE);
  await tick();
  assert.equal(changed, 1);
  watch.touch(WORKSPACE);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(changed, 1, "the same numbers again are not a change");
  git.answer = { added: 0, removed: 0, untracked: 0, ahead: 0 };
  watch.touch(WORKSPACE);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(changed, 2, "a commit taking the numbers to zero is");
  watch.dispose();
});

test("a write inside a subfolder touches the project above it and measures nothing else", async () => {
  const git = counting();
  const watch = new GitChangesWatch(git.read, 1, 10);
  const inside = `${WORKSPACE}${process.platform === "win32" ? "\\packages\\core" : "/packages/core"}`;
  watch.ensure(WORKSPACE);
  await tick();
  assert.equal(git.reads.length, 1);
  watch.touchContaining(inside);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(git.reads, [WORKSPACE, WORKSPACE], "the project was measured again, the subfolder never");
  assert.equal(watch.get(inside), undefined);
  watch.dispose();
});

test("a change under a repository root touches the projects inside it, spelled either way", async () => {
  const git = counting();
  const watch = new GitChangesWatch(git.read, 1, 10);
  const inside = `${WORKSPACE}${process.platform === "win32" ? "\\packages\\core" : "/packages/core"}`;
  const elsewhere = process.platform === "win32" ? "C:\\work\\other" : "/work/other";
  watch.ensure(inside);
  watch.ensure(elsewhere);
  await tick();
  assert.equal(git.reads.length, 2);
  watch.touchUnder(process.platform === "win32" ? WORKSPACE.toUpperCase() : WORKSPACE);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(git.reads.length, 3);
  assert.equal(git.reads[2], inside);
  watch.keep([elsewhere]);
  assert.equal(watch.get(inside), undefined, "a project the list dropped is forgotten");
  watch.dispose();
});
