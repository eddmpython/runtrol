import assert from "node:assert/strict";
import test from "node:test";

import { accentForWorkspace, projectForWorkspace, ProjectStore, type ProjectMemento } from "./projects";
import { projectAccentColor } from "./projectColor";
import { workspaceIdentity } from "./workspaceCollision";

// Built for the platform the tests run on. A project's name is the last path segment, and its
// identity comes from this platform's own separator and casing rules, so a hardcoded backslash is
// an ordinary filename character on Linux and the whole path becomes the "folder name". Measured
// 2026-08-20: these tests were green on Windows and red on the Linux CI runner for that reason.
const ROOT = process.platform === "win32" ? "C:\\work" : "/work";
const SEP = process.platform === "win32" ? "\\" : "/";

const ALPHA = [ROOT, "alpha"].join(SEP);
const BETA = [ROOT, "beta"].join(SEP);

/// A memento that remembers in memory, plus a view of what was persisted.
function memento(initial?: unknown): ProjectMemento & { persisted: () => unknown } {
  const values = new Map<string, unknown>();
  if (initial !== undefined) values.set("runtrol.projects", initial);
  return {
    get: (key) => values.get(key),
    update: (key, value) => {
      values.set(key, value);
      return Promise.resolve();
    },
    persisted: () => values.get("runtrol.projects"),
  };
}

test("creating a project names it after its folder", async () => {
  const store = new ProjectStore(memento());
  const created = await store.create(ALPHA);
  assert.equal(created.name, "alpha");
  assert.equal(created.workspace, ALPHA);
  assert.equal(store.all().length, 1);
});

test("creating the same folder twice is the same project, not an error and not a twin", async () => {
  const store = new ProjectStore(memento());
  const first = await store.create(ALPHA);
  const again = await store.create(ALPHA);
  assert.equal(store.all().length, 1);
  assert.equal(again.key, first.key);
});

test("case follows the platform's own rule, because that is what the folder does", async () => {
  // Two spellings of one name are one folder on Windows and two folders on Linux and macOS. The
  // store must agree with the filesystem it is on: minting one project for two real directories
  // would merge unrelated work, and minting two for one directory shows the operator their project
  // twice. Asserted per platform because the correct answer genuinely differs.
  const store = new ProjectStore(memento());
  await store.create(ALPHA);
  const shouted = await store.create(ALPHA.toUpperCase());
  if (process.platform === "win32") {
    assert.equal(store.all().length, 1, "casing does not mint a second project");
    assert.equal(shouted.key, workspaceIdentity(ALPHA));
  } else {
    assert.equal(store.all().length, 2, "a case-sensitive filesystem has two real folders here");
    assert.notEqual(shouted.key, workspaceIdentity(ALPHA));
  }
});

test("rename keeps the folder and changes only what the person calls it", async () => {
  const store = new ProjectStore(memento());
  await store.create(ALPHA);
  await store.setName(ALPHA, "the real work");
  assert.equal(store.all()[0]?.name, "the real work");
  assert.equal(store.all()[0]?.workspace, ALPHA);
  await assert.rejects(store.setName(ALPHA, "   "), /needs a name/);
});

test("a drag puts the projects in the order it names, and never loses one", async () => {
  const shared = memento();
  const store = new ProjectStore(shared);
  await store.create(ALPHA);
  await store.create(BETA);
  const gamma = await store.create([ROOT, "gamma"].join(SEP));
  assert.deepEqual(store.all().map((record) => record.name), ["alpha", "beta", "gamma"]);

  await store.reorder([gamma.key, workspaceIdentity(ALPHA)]);
  // What the drag named comes first, in its order. What it did not name keeps its place behind them, which is
  // what a drag that started before another window added a project has to do: move what it meant, lose nothing.
  assert.deepEqual(store.all().map((record) => record.name), ["gamma", "alpha", "beta"]);

  // A key that is not a project is not an error and not a gap: it names nothing, so nothing moves for it.
  await store.reorder(["nothing:at:all", workspaceIdentity(BETA)]);
  assert.deepEqual(store.all().map((record) => record.name), ["beta", "gamma", "alpha"]);

  // The order one window drags is the order the next window opens to. The panel is the machine's, not this
  // window's, so an arrangement that lived only here would be a different list in every window.
  const reopened = new ProjectStore(shared);
  assert.deepEqual(reopened.all().map((record) => record.name), ["beta", "gamma", "alpha"]);
});

test("removal takes the heading away and nothing else", async () => {
  const store = new ProjectStore(memento());
  await store.create(ALPHA);
  await store.create(BETA);
  await store.remove(ALPHA);
  assert.deepEqual(store.all().map((row) => row.workspace), [BETA]);
});

test("what one window persists, the next window reads back", async () => {
  const shared = memento();
  await new ProjectStore(shared).create(ALPHA, "named by hand");
  const reopened = new ProjectStore(shared);
  assert.equal(reopened.all()[0]?.name, "named by hand");
  assert.equal(reopened.all()[0]?.key, workspaceIdentity(ALPHA));
});

test("garbage from an older version is dropped, never crashed on", () => {
  const store = new ProjectStore(memento([
    { name: "good", workspace: ALPHA },
    { name: "", workspace: BETA },
    { name: "no folder", workspace: "   " },
    { workspace: BETA },
    "not even an object",
    null,
    { name: "twin of good", workspace: ALPHA.toUpperCase() },
  ]));
  const survivors = store.all();
  if (process.platform === "win32") {
    assert.equal(survivors.length, 1, "the casing twin folded into the first record");
  }
  assert.equal(survivors[0]?.name, "good");
});

test("every change is announced once, after it is persisted", async () => {
  const store = new ProjectStore(memento());
  let announced = 0;
  const subscription = store.onDidChange(() => {
    announced += 1;
  });
  await store.create(ALPHA);
  await store.create(ALPHA);
  await store.setName(ALPHA, "renamed");
  await store.remove(ALPHA);
  assert.equal(announced, 3, "the duplicate create changed nothing and said nothing");
  subscription.dispose();
  await store.create(BETA);
  assert.equal(announced, 3, "a disposed listener hears nothing");
});

test("six colliding projects keep distinct accents across reorder, removal, addition and another window", async () => {
  const shared = memento();
  const store = new ProjectStore(shared);
  const folders = Array.from({ length: 400 }, (_, index) => [ROOT, `project-${index}`].join(SEP));
  const preferred = projectAccentColor(folders[0]!);
  const colliding = folders.filter((folder) => projectAccentColor(folder) === preferred).slice(0, 6);
  assert.equal(colliding.length, 6);
  for (const folder of colliding) await store.create(folder);
  const before = new Map(store.all().map((record) => [record.key, record.accent]));
  assert.equal(new Set(before.values()).size, 6, "path hash collisions do not become visible colour collisions");
  await store.reorder([...before.keys()].reverse());
  await store.setName(colliding[0]!, "renamed");
  await store.setPinned(colliding[1]!, true);
  await store.remove(colliding[5]!);
  await store.create(BETA);
  const reopened = new ProjectStore(shared);
  for (const record of reopened.all()) {
    if (before.has(record.key)) assert.equal(record.accent, before.get(record.key), "open tabs keep their colour");
  }
  assert.equal(new Set(reopened.all().map((record) => record.accent)).size, 6);
});

test("a subfolder tab and its sidebar group share the deepest registered project's accent", async () => {
  const store = new ProjectStore(memento());
  const parent = await store.create(ALPHA);
  const nestedPath = [ALPHA, "nested"].join(SEP);
  const nested = await store.create(nestedPath);
  const child = [nestedPath, "src"].join(SEP);
  assert.equal(projectForWorkspace(store.all(), child)?.key, nested.key);
  assert.equal(accentForWorkspace(store.all(), child), nested.accent);
  assert.equal(accentForWorkspace(store.all(), [ALPHA, "src"].join(SEP)), parent.accent);
  assert.equal(accentForWorkspace(store.all(), null), projectAccentColor(null));
});

test("legacy project accents migrate consistently and survive the first persisted rearrangement", async () => {
  const legacy = [{ name: "alpha", workspace: ALPHA }, { name: "beta", workspace: BETA }];
  const shared = memento(legacy);
  const store = new ProjectStore(shared);
  const reverse = new ProjectStore(memento([...legacy].reverse()));
  for (const record of store.all()) {
    assert.equal(reverse.all().find((other) => other.key === record.key)?.accent, record.accent);
  }
  const before = new Map(store.all().map((record) => [record.key, record.accent]));
  await store.reorder([...before.keys()].reverse());
  for (const record of new ProjectStore(shared).all()) assert.equal(record.accent, before.get(record.key));
});
