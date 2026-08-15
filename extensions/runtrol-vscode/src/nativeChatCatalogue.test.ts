import assert from "node:assert/strict";
import test from "node:test";

import type { NativeSessionCatalogue } from "@runtrol/runtime-client";

import { collectNativeChats, type NativeCatalogueReader } from "./nativeChatCatalogue";

test("official native chats paginate across every approved root and deduplicate identity", async () => {
  const calls: string[] = [];
  const reader: NativeCatalogueReader = {
    async listNativeSessions(params) {
      calls.push(`${params.root}:${params.cursor ?? "first"}`);
      if (params.root === "C:\\one" && !params.cursor) {
        return catalogue([native("one")], "next-one");
      }
      if (params.root === "C:\\one") {
        return catalogue([native("shared")]);
      }
      return catalogue([native("shared"), native("two")]);
    },
  };

  const result = await collectNativeChats(
    reader,
    "provider",
    ["C:\\one", "C:\\two"],
    () => 42,
  );

  assert.deepEqual(calls, ["C:\\one:first", "C:\\one:next-one", "C:\\two:first"]);
  assert.deepEqual(result.chats.map((chat) => chat.nativeSessionId), ["one", "shared", "two"]);
  assert.deepEqual(result.coverage, { kind: "complete", source: "officialCli" });
  assert.equal(result.warning, null);
  assert.equal(result.loadedAtMs, 42);
});

test("mixed support reports partial coverage without hiding supported chats", async () => {
  const reader: NativeCatalogueReader = {
    async listNativeSessions(params) {
      return params.root === "C:\\unsupported"
        ? {
          providerId: "provider",
          coverage: { kind: "unsupported", why: "no official session list" },
          sessions: [],
        }
        : catalogue([native("visible")]);
    },
  };

  const result = await collectNativeChats(
    reader,
    "provider",
    ["C:\\unsupported", "C:\\supported"],
  );

  assert.equal(result.chats[0]?.nativeSessionId, "visible");
  assert.deepEqual(result.coverage, {
    kind: "partial",
    source: "officialCli",
    why: "no official session list",
  });
});

test("discovery failures remain an honest visible catalogue state", async () => {
  const reader: NativeCatalogueReader = {
    async listNativeSessions() {
      throw new Error("provider command unavailable");
    },
  };

  const result = await collectNativeChats(reader, "provider", ["C:\\work"]);

  assert.equal(result.coverage, null);
  assert.deepEqual(result.chats, []);
  assert.match(result.warning ?? "", /provider command unavailable/u);
});

test("a foreground action cancels discovery without publishing a false provider failure", async () => {
  const abort = new AbortController();
  const reader: NativeCatalogueReader = {
    async listNativeSessions() {
      abort.abort(new Error("foreground chat action has priority"));
      return catalogue([native("late")]);
    },
  };

  await assert.rejects(
    collectNativeChats(reader, "provider", ["C:\\work"], Date.now, abort.signal),
    /foreground chat action has priority/u,
  );
});

function catalogue(
  sessions: NativeSessionCatalogue["sessions"],
  nextCursor?: string,
): NativeSessionCatalogue {
  return {
    providerId: "provider",
    coverage: { kind: "complete", source: "officialCli" },
    sessions,
    ...(nextCursor ? { nextCursor } : {}),
  };
}

function native(nativeSessionId: string): NativeSessionCatalogue["sessions"][number] {
  return {
    nativeSessionId,
    cwd: `C:\\work\\${nativeSessionId}`,
    additionalDirectories: [],
    resume: "available",
    adoptionToken: `proof-${nativeSessionId}`,
  };
}
