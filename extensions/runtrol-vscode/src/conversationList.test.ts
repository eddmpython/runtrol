import assert from "node:assert/strict";
import test from "node:test";

import {
  attentionCount,
  conversationDetail,
  conversations,
  elapsed,
  needsYou,
  nextNeedingYou,
  projectDetail,
  projects,
} from "./conversationList";
import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";

const NOW = Date.parse("2026-08-17T12:00:00Z");

const ALPHA = "C:\\work\\alpha";
const BETA = "C:\\work\\beta";
const GAMMA = "C:\\work\\gamma";

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
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: "C:\\work\\alpha" }),
      session({
        sessionId: "s2",
        hot: true,
        lifecycle: "hotRunning",
        waitingOn: "person",
        workspace: "C:\\work\\beta",
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

function waitingFleet(openIndex: number | null) {
  // Five running agents. Two of them stopped for a person, one is throttled, two are simply working.
  const rows = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: "C:\work\a" }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: "C:\work\b" }),
      session({ sessionId: "s3", hot: true, lifecycle: "hotRunning", waitingOn: "quota", workspace: "C:\work\c" }),
      session({ sessionId: "s4", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: "C:\work\d" }),
      session({ sessionId: "s5", hot: true, lifecycle: "hotIdle", workspace: "C:\work\e" }),
    ],
    PROVIDERS,
    [],
    null,
  );
  return { rows, openKey: openIndex === null ? null : rows[openIndex]?.key ?? null };
}

test("one key reaches a conversation that stopped for a person", () => {
  const { rows } = waitingFleet(null);

  const next = nextNeedingYou(rows, null);

  assert.ok(next, "something is waiting");
  assert.equal(needsYou(next), true);
  assert.equal(next.activity, "needsYou");
});

test("pressing it again walks the waiting ones instead of returning to the same one", () => {
  const { rows } = waitingFleet(null);
  const waitingKeys = rows.filter(needsYou).map((row) => row.key);
  assert.equal(waitingKeys.length, 2, "two agents stopped for a person");

  const first = nextNeedingYou(rows, null);
  assert.ok(first);
  const second = nextNeedingYou(rows, first.key);
  assert.ok(second);

  assert.notEqual(first.key, second.key, "the second press moves on");
  assert.deepEqual(new Set([first.key, second.key]), new Set(waitingKeys));
});

test("the walk wraps around rather than dead-ending on the last one", () => {
  const { rows } = waitingFleet(null);
  const waiting = rows.filter(needsYou);
  const last = waiting.at(-1);
  assert.ok(last);

  const wrapped = nextNeedingYou(rows, last.key);

  assert.equal(wrapped?.key, waiting[0]?.key);
});

test("a throttled or working agent is never somewhere the key sends you", () => {
  const { rows } = waitingFleet(null);
  const reachable = new Set<string>();
  let cursor: string | null = null;
  for (let step = 0; step < rows.length + 2; step += 1) {
    const next = nextNeedingYou(rows, cursor);
    if (!next) break;
    reachable.add(next.key);
    cursor = next.key;
  }

  for (const row of rows) {
    if (row.activity === "needsYou") continue;
    assert.equal(reachable.has(row.key), false, `${row.activity} must not be a destination`);
  }
});

test("nothing waiting means nowhere to go, not an arbitrary conversation", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: "C:\work\a" }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotIdle", workspace: "C:\work\b" }),
    ],
    PROVIDERS,
    [],
    null,
  );

  assert.equal(nextNeedingYou(rows, null), null);
  assert.equal(attentionCount(rows), 0);
});

test("a conversation the reader cannot see is not a starting point they get stuck on", () => {
  const { rows } = waitingFleet(null);

  // A key from a conversation that has since left the list.
  const next = nextNeedingYou(rows, "chat gone gone");

  assert.ok(next, "an unknown starting point still lands somewhere useful");
  assert.equal(needsYou(next), true);
});

test("a conversation key is legal as a tree element id", () => {
  // Measured in CI: a NUL separator reads fine as a map key and is not a legal element id. VS Code mangled it,
  // could no longer resolve the element, and every reveal rejected. Fourteen rejections in one run.
  const rows = conversations(
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "ses_1/2 3", workspace: "C:\work\a" })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n 1", providerId: "opencode", cwd: "C:\work\b" })],
    null,
  );

  assert.ok(rows.length >= 2);
  for (const row of rows) {
    assert.ok(row.key.length > 0);
    assert.equal(
      /[ -]/u.test(row.key),
      false,
      `${JSON.stringify(row.key)} carries a control character`,
    );
  }
  assert.equal(new Set(rows.map((row) => row.key)).size, rows.length, "keys stay distinct");
});

/// Enough conversations that project headings apply, spread over the given projects in turn.
///
/// Headings only appear once a flat list stops fitting on screen, so a fixture that wants to exercise them has to
/// be at least that long. Building it here keeps every grouping test honest about which rule it is testing: the
/// length rule, or the one it actually cares about.
function spread(workspaces: readonly string[], count = 6): SessionLine[] {
  return Array.from({ length: count }, (_unused, index) =>
    session({
      sessionId: `s${index}`,
      workspace: workspaces[index % workspaces.length] ?? workspaces[0] ?? "",
    }));
}

test("a project heading appears however short the list is", () => {
  // Project then session, always. An earlier version only grouped past a length threshold, so the sidebar changed
  // shape as conversations accumulated and a single conversation showed no project at all. `runtrol · Claude Code`
  // with no heading does not say which project that is.
  const rows = conversations(spread([ALPHA], 1), PROVIDERS, [], null);
  const groups = projects(rows, [ALPHA]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.name, "alpha");
  assert.equal(groups[0]?.rows.length, 1);
});

test("one project still gets its heading", () => {
  const rows = conversations(spread([ALPHA], 4), PROVIDERS, [], null);
  const groups = projects(rows, [ALPHA]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.rows.length, 4);
});

test("a heading says whether it holds the conversation currently open", () => {
  // A heading that hides the open conversation is a heading that made the reader lose their place, so the tree
  // uses this to arrive expanded.
  const rows = conversations(
    [
      session({ sessionId: "s1", workspace: ALPHA }),
      session({ sessionId: "s2", workspace: BETA }),
    ],
    PROVIDERS,
    [],
    "s2",
  );
  const groups = projects(rows, [ALPHA]);
  assert.equal(groups.find((group) => group.name === "beta")?.holdsOpen, true);
  assert.equal(groups.find((group) => group.name === "alpha")?.holdsOpen, false);
});

test("each project gets a heading and every conversation lands under exactly one", () => {
  const rows = conversations(spread([ALPHA, BETA, GAMMA]), PROVIDERS, [], null);
  const groups = projects(rows, []);
  assert.equal(groups.length, 3);
  const held = groups.flatMap((group) => group.rows.map((row) => row.key));
  assert.equal(held.length, rows.length, "no conversation is dropped");
  assert.equal(new Set(held).size, rows.length, "and none is duplicated");
});

test("this window's project comes first however recently the others were touched", () => {
  // The reader is already in this project. Anything else at the top makes them scroll to where they are.
  const rows = conversations(
    spread([ALPHA, BETA]),
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", cwd: BETA, updatedAt: "2026-08-17T11:59:00Z" })],
    null,
  );
  const groups = projects(rows, [ALPHA]);
  assert.equal(groups[0]?.name, "alpha");
  assert.equal(groups[0]?.current, true);
});

test("heading order does not move when an agent starts or finishes a turn", () => {
  // The regression this ordering exists to prevent. Sorting a heading on what is running inside it would move the
  // heading, and everything under it, every time any agent changed state. A list that rearranges itself while
  // being read is not a list.
  const order = (lifecycle: SessionLine["lifecycle"]) =>
    projects(
      conversations(
        spread([ALPHA, BETA, GAMMA]).map((line, index) =>
          index === 0 ? { ...line, hot: true, lifecycle } : line),
        PROVIDERS,
        [],
        null,
      ),
      [],
    ).map((group) => group.name);
  assert.deepEqual(order("hotIdle"), order("hotRunning"));
});

test("a heading counts what is waiting inside it without moving because of it", () => {
  const lines = spread([ALPHA, BETA]);
  const rows = conversations(
    lines.map((line, index) =>
      index === 0
        ? { ...line, hot: true, lifecycle: "hotRunning" as const, waitingOn: "person" as const }
        : index === 2
          ? { ...line, hot: true, lifecycle: "hotIdle" as const }
          : line),
    PROVIDERS,
    [],
    null,
  );
  const alpha = projects(rows, []).find((group) => group.name === "alpha");
  assert.ok(alpha);
  assert.equal(alpha.attention, 1);
  assert.equal(alpha.live, 2);
  assert.ok(projectDetail(alpha).startsWith("1 waiting"));
});

test("a heading with nothing waiting says only what it holds", () => {
  const rows = conversations(spread([ALPHA, BETA]), PROVIDERS, [], null);
  const beta = projects(rows, []).find((group) => group.name === "beta");
  assert.ok(beta);
  assert.equal(beta.attention, 0);
  assert.ok(projectDetail(beta).endsWith("conversations"));
});

test("the same folder reached two ways is one project, not two", () => {
  // Windows resolves these to the same directory. Two headings for one project would also disagree with
  // collision detection, which already treats them as one working tree.
  if (process.platform !== "win32") {
    // Case folding is a Windows rule. Asserting it elsewhere would assert the opposite of the truth there.
    return;
  }
  const lines = spread([ALPHA, BETA]);
  const rows = conversations(
    lines.map((line, index) => (index === 0 ? { ...line, workspace: "c:\\WORK\\alpha" } : line)),
    PROVIDERS,
    [],
    null,
  );
  const groups = projects(rows, []);
  assert.equal(groups.length, 2, "casing did not split one project in two");
});
