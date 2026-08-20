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

test("a null scope asks about the machine by omitting the folder, not by naming one", async () => {
  // Absence is the request. A provider that filters on a folder it was handed cannot tell "every
  // folder" from "this one", so the field has to be missing rather than empty or null.
  const seen: { hasRoot: boolean; root: unknown }[] = [];
  const reader: NativeCatalogueReader = {
    async listNativeSessions(params) {
      seen.push({ hasRoot: "root" in params, root: params.root });
      return catalogue([native("from-anywhere")]);
    },
  };

  const result = await collectNativeChats(reader, "provider", [null], () => 42);

  assert.deepEqual(seen, [{ hasRoot: false, root: undefined }]);
  assert.deepEqual(result.chats.map((chat) => chat.nativeSessionId), ["from-anywhere"]);
});

test("a machine-wide failure names the machine, not a folder that was never asked about", async () => {
  const reader: NativeCatalogueReader = {
    async listNativeSessions() {
      throw new Error("this provider lists conversations one workspace root at a time");
    },
  };

  const result = await collectNativeChats(reader, "provider", [null], () => 42);

  assert.equal(result.chats.length, 0);
  assert.ok(result.warning?.includes("across this machine"), result.warning ?? "no warning");
  assert.ok(
    result.warning?.includes("one workspace root at a time"),
    "the provider's own reason survives so the caller can act on it",
  );
});
