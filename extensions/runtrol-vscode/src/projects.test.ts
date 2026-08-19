import assert from "node:assert/strict";
import test from "node:test";

import { ProjectStore, type ProjectMemento } from "./projects";
import { workspaceIdentity } from "./workspaceCollision";

const ALPHA = "C:\\work\\alpha";
const BETA = "C:\\work\\beta";

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
  await store.create(ALPHA);
  const again = await store.create("c:\\WORK\\alpha");
  assert.equal(store.all().length, 1);
  if (process.platform === "win32") {
    assert.equal(again.key, workspaceIdentity(ALPHA), "casing does not mint a second project");
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
