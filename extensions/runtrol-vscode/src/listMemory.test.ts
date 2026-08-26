import assert from "node:assert/strict";
import test from "node:test";

import { rememberedList, rememberList, writeRememberedNow } from "./listMemory";
import type { NativeChatCatalogue } from "./runtimeTypes";

/// A memento that behaves like the editor's: synchronous reads, promised writes.
function memento() {
  const store = new Map<string, unknown>();
  return {
    get: <T>(key: string): T | undefined => store.get(key) as T | undefined,
    update: (key: string, value: unknown) => {
      store.set(key, JSON.parse(JSON.stringify(value)));
      return Promise.resolve();
    },
    keys: () => [...store.keys()],
    setKeysForSync: () => undefined,
  };
}

function catalogue(providerId: string, count: number): NativeChatCatalogue {
  return {
    providerId,
    chats: Array.from({ length: count }, (_unused, index) => ({
      providerId,
      nativeSessionId: `${providerId}-${index}`,
      title: `Chat ${index}`,
      cwd: "C:/work",
      updatedAt: "1700000000000",
      resume: "available",
    })),
  } as unknown as NativeChatCatalogue;
}

test("what one window drew is what the next window draws first", async () => {
  const store = memento();
  rememberList(store as never, [catalogue("claude", 2), catalogue("grok", 1)]);
  await writeRememberedNow();
  const back = rememberedList(store as never);
  assert.deepEqual(back.map((entry) => [entry.providerId, entry.chats.length]), [["claude", 2], ["grok", 1]]);
});

test("nothing remembered, and anything unrecognisable, draws no rows instead of wrong ones", async () => {
  const empty = memento();
  assert.deepEqual(rememberedList(empty as never), []);

  // Written by an older build whose rows had different fields. Drawing those would put half-formed rows on
  // screen, which is worse than the wait this exists to remove.
  const stale = memento();
  await stale.update("runtrol.listMemory.v1", { catalogues: [{ providerId: "claude", chats: [{ id: "x" }] }] });
  assert.deepEqual(rememberedList(stale as never), []);

  const wrong = memento();
  await wrong.update("runtrol.listMemory.v1", { catalogues: "everything" });
  assert.deepEqual(rememberedList(wrong as never), []);
});

test("the remembered list is bounded, so a settings file cannot grow without end", async () => {
  const store = memento();
  rememberList(store as never, [catalogue("claude", 500), catalogue("codex", 500)]);
  await writeRememberedNow();
  const kept = rememberedList(store as never).reduce((total, entry) => total + entry.chats.length, 0);
  assert.equal(kept, 600);
});
