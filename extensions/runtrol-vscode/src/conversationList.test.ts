import assert from "node:assert/strict";
import test from "node:test";

import {
  attentionCount,
  conversationDetail,
  conversations,
  elapsed,
  loose,
  needsYou,
  nextNeedingYou,
  projectDetail,
  projects,
} from "./conversationList";
import type { ProjectRecord } from "./projects";
import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { workspaceIdentity } from "./workspaceCollision";

const NOW = Date.parse("2026-08-17T12:00:00Z");

// Paths here are built for the platform the tests run on. The product compares workspaces with
// that platform's own rules (`workspaceCovers` takes its separator and casing from it), so a
// hardcoded backslash is a folder name on Linux rather than a separator, and containment tests
// that pass on Windows fail in CI. Measured 2026-08-20: four were red on the Linux runner and
// green on every developer machine.
const ROOT = process.platform === "win32" ? "C:\\work" : "/work";
const SEP = process.platform === "win32" ? "\\" : "/";

/// One path below another, in the separator this platform actually uses.
function below(base: string, ...parts: readonly string[]): string {
  return [base, ...parts].join(SEP);
}

const ALPHA = below(ROOT, "alpha");
const BETA = below(ROOT, "beta");
const GAMMA = below(ROOT, "gamma");

/// A project the operator created on this folder, the way the store would record it.
function record(workspace: string, name?: string): ProjectRecord {
  return {
    key: workspaceIdentity(workspace),
    name: name ?? workspace.split(/[\\/]/).pop() ?? workspace,
    workspace,
  };
}

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
    workspace: ALPHA,
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
    cwd: BETA,
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
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: BETA })],
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
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: BETA })],
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
      session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", workspace: ALPHA }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotIdle", workspace: BETA }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const working = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", workspace: ALPHA }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", workspace: BETA }),
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
      session({ sessionId: "s1", workspace: ALPHA }),
      session({ sessionId: "s2", workspace: ALPHA }),
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
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: ALPHA }),
      session({
        sessionId: "s2",
        hot: true,
        lifecycle: "hotRunning",
        waitingOn: "person",
        workspace: BETA,
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
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: below(ROOT, "a") }),
      session({ sessionId: "s2", hot: true, lifecycle: "failed", workspace: below(ROOT, "b") }),
      session({ sessionId: "s3", hot: true, lifecycle: "hotRunning", waitingOn: "quota", workspace: below(ROOT, "c") }),
      session({ sessionId: "s4", hot: true, lifecycle: "hotRunning", workspace: below(ROOT, "d") }),
      session({ sessionId: "s5", hot: true, lifecycle: "hotIdle", workspace: below(ROOT, "e") }),
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
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: below(ROOT, "a") }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: below(ROOT, "b") }),
      session({ sessionId: "s3", hot: true, lifecycle: "hotRunning", waitingOn: "quota", workspace: below(ROOT, "c") }),
      session({ sessionId: "s4", hot: true, lifecycle: "hotRunning", waitingOn: "person", workspace: below(ROOT, "d") }),
      session({ sessionId: "s5", hot: true, lifecycle: "hotIdle", workspace: below(ROOT, "e") }),
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
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: below(ROOT, "a") }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotIdle", workspace: below(ROOT, "b") }),
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
  const next = nextNeedingYou(rows, "chat:gone:gone");

  assert.ok(next, "an unknown starting point still lands somewhere useful");
  assert.equal(needsYou(next), true);
});

test("a conversation key is legal as a tree element id", () => {
  // Measured in CI: a NUL separator reads fine as a map key and is not a legal element id. VS Code mangled it,
  // could no longer resolve the element, and every reveal rejected. Fourteen rejections in one run.
  const rows = conversations(
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "ses_1/2 3", workspace: below(ROOT, "a") })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n 1", providerId: "opencode", cwd: below(ROOT, "b") })],
    null,
  );

  assert.ok(rows.length >= 2);
  for (const row of rows) {
    assert.ok(row.key.length > 0);
    assert.equal(
      /[\x00-\x1f\x7f]/u.test(row.key),
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




test("a conversation nobody named still reads as something", () => {
  // With no title from the service and no folder to borrow a name from, the label was empty. Composing one out of
  // what was said would mean reading the conversation, so the identifier the service did give is used, and it is
  // what tells two of these apart.
  const rows = conversations(
    [
      session({ sessionId: "01a011e6-414e-7601-9f70-2f66980e2acd", workspace: "" }),
      session({ sessionId: "fc2e97a4-1030-43fe-ae32-e78e79351ce1", workspace: "" }),
    ],
    PROVIDERS,
    [],
    null,
  );
  assert.equal(rows.length, 2);
  for (const row of rows) {
    assert.ok(row.title.startsWith("Untitled · "), `${row.title} names nothing`);
  }
  assert.notEqual(rows[0]?.title, rows[1]?.title, "two nameless conversations are still told apart");
});


test("a conversation with no folder is not filed under an invented one", () => {
  // Measured across four coding services: a conversation can arrive carrying no working directory. It used to
  // collapse into a heading whose label was empty, because resolving "" returns the Extension Host's own
  // directory. Naming that heading "No project" was no better: it turns an absence into a folder the reader
  // thinks they forgot about. The chat apps people already use do neither. A conversation nobody filed is a
  // conversation, and it sits at the top level beside the projects.
  const rows = conversations(
    [session({ sessionId: "a", workspace: ALPHA }), session({ sessionId: "b", workspace: "" })],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA)], rows, [ALPHA]);
  assert.equal(groups.length, 1, "only the created project is a heading");
  assert.equal(groups[0]?.name, "alpha");
  const unfiled = loose([record(ALPHA)], rows, []);
  assert.equal(unfiled.length, 1);
  assert.equal(unfiled[0]?.session?.sessionId, "b");
});

test("a folder that is only whitespace files nowhere", () => {
  const rows = conversations([session({ sessionId: "a", workspace: "   " })], PROVIDERS, [], null);
  const filed = projects([record(ALPHA)], rows, []).flatMap((group) => group.rows);
  assert.equal(filed.length, 0);
  assert.equal(loose([record(ALPHA)], rows, []).length, 1);
});

test("a folder becomes a heading only when somebody created it or has it open", () => {
  // The regression this shape exists to prevent, in both directions. The panel once invented a heading for
  // every folder any conversation had run in (measured: thirty auto-headings named workspace-1 through
  // workspace-30, rejected by the operator on sight), and the correction then over-shot and flattened even
  // the window's own open folder into rows that repeated its name (rejected again, 2026-08-20). Opening a
  // folder is the operator's act exactly like creating a project: the open folder is a heading, everything
  // neither created nor open stays a plain row.
  const rows = conversations(spread([ALPHA, BETA, GAMMA]), PROVIDERS, [], null);
  const groups = projects([], rows, [ALPHA]);
  assert.equal(groups.length, 1, "the open folder is the one heading");
  assert.equal(groups[0]?.name, "alpha");
  assert.equal(groups[0]?.current, true, "the open folder is this window's project");
  assert.equal(groups[0]?.rows.length, 2, "its own conversations file under it");
  const unfiled = loose([], rows, [ALPHA]);
  assert.equal(unfiled.length, rows.length - 2, "folders nobody created or opened stay plain rows");
  assert.equal(projects([], rows, []).length, 0, "with nothing created and nothing open, no headings");
});

test("a created project standing on the open folder draws the one heading", () => {
  const rows = conversations(spread([ALPHA]), PROVIDERS, [], null);
  const groups = projects([record(ALPHA)], rows, [ALPHA]);
  assert.equal(groups.length, 1, "one place, one heading");
  assert.equal(groups[0]?.key.startsWith("project:"), true, "creation is the more deliberate act");
});

test("a grouped row does not repeat the folder its heading already names", () => {
  const rows = conversations(spread([ALPHA]), PROVIDERS, [], null);
  const row = rows[0];
  assert.ok(row);
  assert.ok(conversationDetail(row, Date.now()).includes(row.folder));
  assert.ok(!conversationDetail(row, Date.now(), true).includes(row.folder));
});

test("a created project with no conversations yet still shows its heading", () => {
  // It was made a moment ago. A heading that vanished until a conversation arrived would read as the creation
  // having failed.
  const groups = projects([record(ALPHA)], [], []);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.rows.length, 0);
  assert.ok(groups[0]);
  assert.equal(projectDetail(groups[0]), "no conversations yet");
});

test("a conversation in a subfolder files under the project that covers it", () => {
  const rows = conversations(
    [session({ sessionId: "deep", workspace: `${ALPHA}\\packages\\core` })],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA)], rows, []);
  assert.equal(groups[0]?.rows.length, 1);
  assert.equal(loose([record(ALPHA)], rows, []).length, 0);
});

test("nested projects file a conversation under the deepest one", () => {
  const sub = `${ALPHA}\\packages\\core`;
  const rows = conversations(
    [
      session({ sessionId: "inner", workspace: `${sub}\\src` }),
      session({ sessionId: "outer", workspace: `${ALPHA}\\docs` }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA), record(sub, "core")], rows, []);
  const outer = groups.find((group) => group.name === "alpha");
  const inner = groups.find((group) => group.name === "core");
  assert.equal(inner?.rows[0]?.session?.sessionId, "inner");
  assert.equal(outer?.rows[0]?.session?.sessionId, "outer");
});

test("every conversation is either in a project or loose, never both and never neither", () => {
  // The two functions split one list, so a row falling through both would vanish from the tree with nothing
  // saying so, and a row in both would be drawn twice.
  const rows = conversations(
    [
      session({ sessionId: "a", workspace: ALPHA }),
      session({ sessionId: "b", workspace: BETA }),
      session({ sessionId: "c", workspace: "" }),
      session({ sessionId: "d", workspace: ALPHA }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const records = [record(ALPHA), record(BETA)];
  const filed = projects(records, rows, []).flatMap((group) => group.rows);
  const unfiled = loose(records, rows, []);
  assert.equal(filed.length + unfiled.length, rows.length);
  const seen = new Set([...filed, ...unfiled].map((row) => row.key));
  assert.equal(seen.size, rows.length, "no row is drawn twice");
});

test("a conversation nobody named still reads as something", () => {
  // With no title from the service and no folder to borrow a name from, the label was empty. Composing one out
  // of what was said would mean reading the conversation, so the identifier the service did give is used, and it
  // is what tells two of these apart.
  const rows = conversations(
    [
      session({ sessionId: "01a011e6-414e-7601-9f70-2f66980e2acd", workspace: "" }),
      session({ sessionId: "fc2e97a4-1030-43fe-ae32-e78e79351ce1", workspace: "" }),
    ],
    PROVIDERS,
    [],
    null,
  );
  for (const row of rows) {
    assert.ok(row.title.startsWith("Untitled · "), `${row.title} names nothing`);
  }
  assert.notEqual(rows[0]?.title, rows[1]?.title, "two nameless conversations are still told apart");
});

test("a created project shows its heading however short the list is", () => {
  // An earlier folder-derived version only grouped past a length threshold, so the sidebar changed shape as
  // conversations accumulated. A created project is a decision, and it shows from its first conversation.
  const rows = conversations(spread([ALPHA], 1), PROVIDERS, [], null);
  const groups = projects([record(ALPHA)], rows, [ALPHA]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.name, "alpha");
  assert.equal(groups[0]?.rows.length, 1);
});

test("one created project still gets its heading", () => {
  const rows = conversations(spread([ALPHA], 4), PROVIDERS, [], null);
  const groups = projects([record(ALPHA)], rows, [ALPHA]);
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
  const groups = projects([record(ALPHA), record(BETA)], rows, [ALPHA]);
  assert.equal(groups.find((group) => group.name === "beta")?.holdsOpen, true);
  assert.equal(groups.find((group) => group.name === "alpha")?.holdsOpen, false);
});

test("each created project gets a heading and every conversation lands under exactly one", () => {
  const rows = conversations(spread([ALPHA, BETA, GAMMA]), PROVIDERS, [], null);
  const groups = projects([record(ALPHA), record(BETA), record(GAMMA)], rows, []);
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
  const groups = projects([record(ALPHA), record(BETA)], rows, [ALPHA]);
  assert.equal(groups[0]?.name, "alpha");
  assert.equal(groups[0]?.current, true);
});

test("heading order does not move when an agent starts or finishes a turn", () => {
  // The regression this ordering exists to prevent. Sorting a heading on what is running inside it would move the
  // heading, and everything under it, every time any agent changed state. A list that rearranges itself while
  // being read is not a list.
  const order = (lifecycle: SessionLine["lifecycle"]) =>
    projects(
      [record(ALPHA), record(BETA), record(GAMMA)],
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
  const alpha = projects([record(ALPHA), record(BETA)], rows, []).find((group) => group.name === "alpha");
  assert.ok(alpha);
  assert.equal(alpha.attention, 1);
  assert.equal(alpha.live, 2);
  assert.ok(projectDetail(alpha).startsWith("1 waiting"));
});

test("a heading with nothing waiting says only what it holds", () => {
  const rows = conversations(spread([ALPHA, BETA]), PROVIDERS, [], null);
  const beta = projects([record(ALPHA), record(BETA)], rows, []).find((group) => group.name === "beta");
  assert.ok(beta);
  assert.equal(beta.attention, 0);
  assert.ok(projectDetail(beta).endsWith("conversations"));
});

test("the same folder reached two ways files into one project", () => {
  // Windows resolves these to the same directory. A conversation whose recorded folder differs from the
  // project's only by casing still belongs to it, because collision detection already treats them as one
  // working tree.
  if (process.platform !== "win32") {
    // Case folding is a Windows rule. Asserting it elsewhere would assert the opposite of the truth there.
    return;
  }
  const rows = conversations(
    [session({ sessionId: "cased", workspace: "c:\\WORK\\alpha" })],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA)], rows, []);
  assert.equal(groups[0]?.rows.length, 1, "casing did not push the conversation out of its project");
  assert.equal(loose([record(ALPHA)], rows, []).length, 0);
});
