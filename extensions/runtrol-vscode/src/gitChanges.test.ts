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
  const refused = () => Promise.reject(Object.assign(new Error("git failed"), {
    code: 128, stderr: "fatal: not a git repository (or any of the parent directories): .git\n",
  }));
  assert.equal(await readGitChanges(WORKSPACE, refused), null);
});

test("a failed Git read stays distinct from a repository with no first commit", async () => {
  const failure = Object.assign(new Error("Git read timed out"), { killed: true, signal: "SIGTERM" });
  await assert.rejects(readGitChanges(WORKSPACE, async () => { throw failure; }), (error) => error === failure);
  assert.equal(await readGitChanges(WORKSPACE, async (_workspace, args) => {
    if (args[0] === "status") return "# branch.oid (initial)\n# branch.head main\n? first.ts\n";
    throw new Error("HEAD does not exist");
  }), null);
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
  const watch = new GitChangesWatch(git.read, 5, 50, 0);
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
  const watch = new GitChangesWatch(git.read, 80, 250, 0);
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

test("a folder becomes measurable after its first commit without leaving the list", async () => {
  const git = counting();
  git.answer = null as unknown as GitChanges;
  const watch = new GitChangesWatch(git.read, 1, 10, 0);
  watch.ensure(WORKSPACE);
  await tick();
  assert.equal(git.reads.length, 1);
  git.answer = { added: 4, removed: 0, untracked: 1, ahead: 0 };
  watch.touch(WORKSPACE);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(git.reads.length, 2, "a real change signal can recover an unborn or newly initialized repository");
  assert.deepEqual(watch.get(WORKSPACE), git.answer);
  watch.dispose();
});

test("a slow measurement coalesces new signals and never overlaps another read of the same project", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000 });
  const pending: ((changes: GitChanges) => void)[] = [];
  const watch = new GitChangesWatch(() => new Promise((resolve) => pending.push(resolve)), 1, 10, 5);
  try {
    watch.ensure(WORKSPACE);
    for (let index = 0; index < 10; index += 1) {
      watch.touch(WORKSPACE);
      context.mock.timers.tick(10);
    }
    assert.equal(pending.length, 1, "signals while Git is slow cannot start competing subprocesses");
    const first = { added: 1, removed: 0, untracked: 0, ahead: 0 };
    pending[0]!(first);
    await Promise.resolve();
    context.mock.timers.tick(1);
    assert.equal(pending.length, 2, "one follow-up measures changes that arrived during the first read");
    const second = { ...first, added: 2 };
    pending[1]!(second);
    await Promise.resolve();
    assert.deepEqual(watch.get(WORKSPACE), second);
    context.mock.timers.tick(100);
    assert.equal(pending.length, 2, "idle projects do not poll");
  } finally {
    watch.dispose();
    context.mock.timers.reset();
  }
});

test("a transient failure is visible and an explicit refresh recovers without polling", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000 });
  let reads = 0;
  const answer = { added: 3, removed: 1, untracked: 0, ahead: 0 };
  const watch = new GitChangesWatch(async () => {
    reads += 1;
    if (reads === 1) throw new Error("Git read timed out");
    return answer;
  }, 1, 10, 5);
  try {
    watch.ensure(WORKSPACE);
    await Promise.resolve();
    assert.equal(watch.get(WORKSPACE), null);
    assert.equal(watch.getError(WORKSPACE), "Git read timed out");
    context.mock.timers.tick(100);
    assert.equal(reads, 1);
    watch.refresh();
    context.mock.timers.tick(1);
    await Promise.resolve();
    assert.deepEqual(watch.get(WORKSPACE), answer);
    assert.equal(watch.getError(WORKSPACE), null);
  } finally {
    watch.dispose();
    context.mock.timers.reset();
  }
});

test("removing a project cancels its scheduled read and discards an older in-flight result", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000 });
  const pending: ((changes: GitChanges) => void)[] = [];
  const watch = new GitChangesWatch(() => new Promise((resolve) => pending.push(resolve)), 1, 10, 0);
  try {
    watch.ensure(WORKSPACE);
    watch.touch(WORKSPACE);
    watch.keep([]);
    watch.ensure(WORKSPACE);
    assert.equal(pending.length, 2);
    const current = { added: 2, removed: 0, untracked: 0, ahead: 0 };
    pending[1]!(current);
    await Promise.resolve();
    pending[0]!({ ...current, added: 1 });
    await Promise.resolve();
    context.mock.timers.tick(100);
    assert.equal(pending.length, 2, "the removed incarnation leaves no scheduled subprocess");
    assert.deepEqual(watch.get(WORKSPACE), current, "an older incarnation cannot overwrite the current row");
  } finally {
    watch.dispose();
    context.mock.timers.reset();
  }
});

test("two touches inside the floor become one measurement after it", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000 });
  const git = counting();
  const watch = new GitChangesWatch(git.read, 1, 10, 40);
  try {
    watch.ensure(WORKSPACE);
    // Complete the first read without moving the floor's clock. A real zero-delay timer can resume after it.
    await Promise.resolve();
    assert.equal(git.reads.length, 1);
    watch.touch(WORKSPACE);
    context.mock.timers.tick(5);
    watch.touch(WORKSPACE);
    context.mock.timers.tick(34);
    assert.equal(git.reads.length, 1, "inside the floor nothing runs");
    context.mock.timers.tick(1);
    await Promise.resolve();
    assert.equal(git.reads.length, 2, "at the floor both touches are honoured by one read");
    context.mock.timers.tick(40);
    assert.equal(git.reads.length, 2, "the coalesced touch leaves no extra measurement");
  } finally {
    watch.dispose();
    context.mock.timers.reset();
  }
});

test("an unchanged answer does not redraw, a changed one does", async () => {
  const git = counting();
  const watch = new GitChangesWatch(git.read, 1, 10, 0);
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
  const watch = new GitChangesWatch(git.read, 1, 10, 0);
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
  const watch = new GitChangesWatch(git.read, 1, 10, 0);
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
