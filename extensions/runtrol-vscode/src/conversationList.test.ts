import assert from "node:assert/strict";
import test from "node:test";

import { attentionCount, conversationDetail, conversations, elapsed, needsYou } from "./conversationList";
import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";

const NOW = Date.parse("2026-08-17T12:00:00Z");

const PROVIDERS: ProviderLine[] = [
  { providerId: "claude", displayName: "Claude Code", installation: { state: "usable", version: "2.1.0" } },
  { providerId: "codex", displayName: "Codex", installation: { state: "usable", version: "0.9.0" } },
  { providerId: "opencode", displayName: "OpenCode", installation: { state: "usable", version: "1.2.27" } },
];

function session(overrides: Partial<SessionLine> & Pick<SessionLine, "sessionId">): SessionLine {
  return {
    providerId: "claude",
    nativeSessionId: null,
    label: null,
    workspace: "C:\\work\\alpha",
    hot: false,
    lifecycle: "cold",
    looksStuck: false,
    sessionGeneration: 1,
    ...overrides,
  } as SessionLine;
}

function nativeChat(overrides: Partial<NativeChatLine> & Pick<NativeChatLine, "nativeSessionId">): NativeChatLine {
  return {
    providerId: "codex",
    cwd: "C:\\work\\beta",
    additionalDirectories: [],
    title: null,
    updatedAt: null,
    resume: "available",
    alreadyManagedAs: null,
    adoptionToken: "token",
    ...overrides,
  } as NativeChatLine;
}

test("one list holds supervised sessions and provider-owned chats alike", () => {
  const rows = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotIdle" })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", title: "Refactor the parser" })],
    null,
  );

  assert.equal(rows.length, 2);
  assert.equal(rows[0]?.live, true, "a live conversation leads");
  assert.equal(rows[1]?.title, "Refactor the parser");
  assert.equal(rows[1]?.serviceName, "Codex");
});

test("a supervised session and the chat it came from are one row, not two", () => {
  const rows = conversations(
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: "C:\\work\\beta" })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", title: "Refactor the parser", updatedAt: "2026-08-17T11:30:00Z" })],
    null,
  );

  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.session?.sessionId, "s1", "the supervised half wins");
  assert.equal(rows[0]?.title, "Refactor the parser", "and still borrows the service title");
  assert.equal(rows[0]?.updatedAtMs, Date.parse("2026-08-17T11:30:00Z"), "and its timestamp");
});

test("opening a saved chat keeps the same row identity", () => {
  const before = conversations([], PROVIDERS, [nativeChat({ nativeSessionId: "n1" })], null);
  const after = conversations(
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: "C:\\work\\beta" })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", alreadyManagedAs: "s1" })],
    "s1",
  );

  assert.equal(before.length, 1);
  assert.equal(after.length, 1);
  assert.equal(before[0]?.key, after[0]?.key, "the row updates in place instead of jumping");
  assert.equal(after[0]?.open, true);
});

test("turn state never reorders the list", () => {
  const idle = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", workspace: "C:\\work\\alpha" }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotIdle", workspace: "C:\\work\\beta" }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const working = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", workspace: "C:\\work\\alpha" }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", workspace: "C:\\work\\beta" }),
    ],
    PROVIDERS,
    [],
    null,
  );

  assert.deepEqual(idle.map((row) => row.key), working.map((row) => row.key));
  assert.equal(working[1]?.activity, "working");
});

test("a chat the service cannot reopen says so instead of failing on click", () => {
  const rows = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", resume: "unavailable", adoptionToken: null })],
    null,
  );

  assert.equal(rows[0]?.canOpen, false);
  assert.ok(rows[0]?.blocked);
});

test("a stuck session asks for attention without leaving its place", () => {
  const rows = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", looksStuck: true })],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(rows[0]?.activity, "attention");
});

test("rows a person could not tell apart get an identity", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", workspace: "C:\\work\\alpha" }),
      session({ sessionId: "s2", workspace: "C:\\work\\alpha" }),
    ],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(new Set(rows.map((row) => row.title)).size, 2);
});

test("the detail line drops a folder that only repeats the title", () => {
  const [plain] = conversations([session({ sessionId: "s1" })], PROVIDERS, [], null);
  const [named] = conversations([session({ sessionId: "s2", label: "Nightly sweep" })], PROVIDERS, [], null);

  assert.ok(plain);
  assert.ok(named);
  assert.equal(conversationDetail(plain, NOW), "Claude Code");
  assert.equal(conversationDetail(named, NOW), "alpha · Claude Code");
});

test("elapsed time reads the way a chat list writes it", () => {
  assert.equal(elapsed(null, NOW), null);
  assert.equal(elapsed(NOW - 5_000, NOW), "now");
  assert.equal(elapsed(NOW - 5 * 60_000, NOW), "5m");
  assert.equal(elapsed(NOW - 3 * 3_600_000, NOW), "3h");
  assert.equal(elapsed(NOW - 2 * 86_400_000, NOW), "2d");
  assert.equal(elapsed(NOW - 30 * 86_400_000, NOW), "4w");
});

test("a turn that stopped for a person outranks one that is merely working", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: "C:\work\alpha" }),
      session({
        sessionId: "s2",
        hot: true,
        lifecycle: "hotRunning",
        waitingOn: "person",
        workspace: "C:\work\beta",
      }),
    ],
    PROVIDERS,
    [],
    null,
  );

  const working = rows.find((row) => row.session?.sessionId === "s1");
  const blocked = rows.find((row) => row.session?.sessionId === "s2");
  assert.equal(working?.activity, "working");
  assert.equal(blocked?.activity, "needsYou", "a running turn that stopped for a person is not just working");
});

test("a quota wait is not an errand for the reader", () => {
  const [row] = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", waitingOn: "quota" })],
    PROVIDERS,
    [],
    null,
  );

  assert.ok(row);
  assert.equal(row.activity, "waitingOnQuota");
  assert.equal(needsYou(row), false, "nobody can answer a rate limit, so it must not ask to be answered");
});

test("a broken session outranks a waiting one, because it cannot be answered either", () => {
  const [row] = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "failed", waitingOn: "person" })],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(row?.activity, "attention");
});

test("the badge counts exactly the conversations a person has to act on", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: "C:\work\a" }),
      session({ sessionId: "s2", hot: true, lifecycle: "failed", workspace: "C:\work\b" }),
      session({ sessionId: "s3", hot: true, lifecycle: "hotRunning", waitingOn: "quota", workspace: "C:\work\c" }),
      session({ sessionId: "s4", hot: true, lifecycle: "hotRunning", workspace: "C:\work\d" }),
      session({ sessionId: "s5", hot: true, lifecycle: "hotIdle", workspace: "C:\work\e" }),
    ],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(attentionCount(rows), 2, "one waiting and one broken, and nothing else");
});

test("a waiting state never survives its own turn", () => {
  // Runtime clears it, and the projection must not resurrect it from a stale field on a cold row.
  const [row] = conversations(
    [session({ sessionId: "s1", hot: false, lifecycle: "cold", waitingOn: null })],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(row?.activity, "saved");
  assert.equal(needsYou(row!), false);
});
