import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import test from "node:test";

import {
  attentionCount,
  conversationDetail,
  runningElsewhere,
  namedPlaceholders,
  conversationStatus,
  conversations,
  elapsed,
  loose,
  nativeProcessKey,
  needsYou,
  nextNeedingYou,
  projectDetail,
  projects,
} from "./conversationList";
import type { ProjectRecord } from "./projects";
import type { NativeChatLine, ProviderLine, SessionLine, TerminalDescriptor } from "./runtimeTypes";
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
    pinned: false,
  };
}

const PROVIDERS: ProviderLine[] = [
  { providerId: "claude", displayName: "Claude Code", installation: { state: "usable", version: "2.1.0" } },
  { providerId: "codex", displayName: "Codex", installation: { state: "usable", version: "0.9.0" } },
  { providerId: "acp-fixture", displayName: "ACP Fixture", installation: { state: "usable", version: "1.2.27" } },
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

test("an open TUI repaint stays idle until the provider proves a model turn", () => {
  // An idle or paused TUI writes prompts, menus and cursor updates. Those bytes keep the terminal live and
  // openable but must never turn the sidebar icon (operator, 2026-08-31).
  const idle = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", nativeSessionId: "n1", providerId: "claude" })],
    [],
    [],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
  );
  assert.equal(idle[0]?.activity, "ready");
  const key = idle[0]?.key ?? "";
  const repainting = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", nativeSessionId: "n1", providerId: "claude" })],
    [],
    [],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set([key]),
  );
  assert.equal(repainting[0]?.activity, "ready", "screen output alone is not model activity");
  const working = conversations(
    [session({ sessionId: "s1", hot: true, lifecycle: "hotIdle", nativeSessionId: "n1", providerId: "claude" })],
    [],
    [],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set([nativeProcessKey("claude", "n1")]),
  );
  assert.equal(working[0]?.activity, "working", "the provider's open turn animates the row");
});

test("a conversation Runtrol does not host follows the provider's structural turn state", () => {
  // The same provider-owned turn signal covers external and hosted terminals without reading either screen.
  const idle = conversations([], PROVIDERS, [nativeChat({ nativeSessionId: "n9" })], null);
  assert.equal(idle[0]?.activity, "saved");
  const busy = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set([nativeProcessKey("codex", "n9")]),
  );
  assert.equal(busy[0]?.activity, "working");
  // Named by the service's own identity, not by the row key: the answer comes from the service's store.
  const other = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set([nativeProcessKey("codex", "someone-else")]),
  );
  assert.equal(other[0]?.activity, "saved");
});

test("an externally running provider conversation without a route is live but never resumed", () => {
  const key = nativeProcessKey("codex", "n9");
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9", title: "Already running elsewhere" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([key]),
  );

  assert.equal(row?.live, true);
  assert.equal(row?.canOpen, false, "clicking must not create a duplicate resume process");
  // The sentence is what the row's tooltip says and what the click repeats, so it is written for a person:
  // where the conversation is, and why this panel will not open it.
  assert.match(row?.blocked ?? "", /already running, but its live terminal is not available/u);
  // The click path asks this rather than reading the sentence, so the words and the behaviour cannot drift.
  assert.equal(row ? runningElsewhere(row) : false, true);
});

test("an externally running terminal with a proven route opens without a second owner", () => {
  const key = nativeProcessKey("claude", "n9");
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ providerId: "claude", nativeSessionId: "n9", title: "Attach here" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([key]),
    [],
    new Set(),
    new Set([key]),
  );

  assert.equal(row?.presence.kind, "external");
  assert.equal(row?.live, true);
  assert.equal(row?.canOpen, true, "the click must ask Runtime to attach the exact live owner");
  assert.equal(row?.blocked, null);
  assert.equal(row ? runningElsewhere(row) : true, false);
});

test("a conversation running in a window's own terminal offers that window, and never an open", () => {
  const key = nativeProcessKey("claude", "n11");
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ providerId: "claude", nativeSessionId: "n11", title: "Typed in another window" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([key]),
    [],
    new Set(),
    // Not attachable: with no shell integration there is no route to attach, only the window that owns it.
    new Set(),
    new Set([key]),
  );

  assert.equal(row?.live, true);
  assert.equal(row?.canOpen, false, "focus is never a way to open a second owner");
  assert.equal(row?.canFocus, true);
  assert.equal(row?.blocked, null, "a row that can be shown where it runs is not blocked");
});

test("a live conversation nothing can reach says only that much", () => {
  const key = nativeProcessKey("claude", "n12");
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ providerId: "claude", nativeSessionId: "n12", title: "Somewhere else entirely" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([key]),
    [],
    new Set(),
    new Set(),
    new Set(),
  );

  assert.equal(row?.live, true);
  assert.equal(row?.canOpen, false);
  assert.equal(row?.canFocus, false);
  assert.equal(row ? runningElsewhere(row) : false, true);
});

test("only a conversation running outside Runtrol answers to running elsewhere", () => {
  // Every other reason a row cannot open leaves it not live, which is what makes the one predicate exact.
  const [saved] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", adoptionToken: null })],
    null,
    null,
  );
  assert.equal(saved?.live, false);
  assert.equal(saved?.canOpen, false, "a service that cannot resume still shows the conversation");
  assert.equal(saved ? runningElsewhere(saved) : true, false);
});

test("a failed live roster check revokes Elsewhere without allowing a duplicate owner", () => {
  const key = nativeProcessKey("codex", "n9");
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9", title: "Owner status unavailable" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set(),
    [],
    new Set([key]),
  );

  assert.equal(row?.presence.kind, "unconfirmed");
  assert.equal(row?.live, false, "an unavailable roster is not current proof of a live process");
  assert.equal(row?.canOpen, false, "uncertainty must not start a second process owner");
  assert.equal(row ? runningElsewhere(row) : true, false, "only current live proof earns Elsewhere");
  assert.match(row?.blocked ?? "", /could not confirm whether this conversation is still running/u);
});

test("a daemon-owned terminal is the exact attach target and keeps the provider title", () => {
  const terminal = {
    terminalId: "terminal-1",
    runtimeGeneration: "generation-1",
    providerId: "codex",
    workspace: BETA,
    nativeSessionId: "n9",
    processState: "running",
    openedAtMs: NOW,
    terminalGeneration: 7,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: 1024,
  } as TerminalDescriptor;
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9", title: "Provider supplied title" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set(),
    [terminal],
  );

  assert.equal(row?.title, "Provider supplied title");
  assert.equal(row?.hostedTerminal, terminal);
  assert.equal(row?.canOpen, true);
});

test("a terminal an earlier Runtime generation still owns opens there, even while the roster sees its process", () => {
  // An update leaves the old generation draining with this conversation's PTY. The provider's own roster keeps
  // naming the process alive. The row must attach in that generation, not refuse as running outside Runtrol.
  const terminal = {
    terminalId: "terminal-old",
    runtimeGeneration: "generation-old",
    providerId: "codex",
    workspace: BETA,
    nativeSessionId: "n9",
    processState: "running",
    openedAtMs: NOW - 1_000,
    terminalGeneration: 3,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor;
  const [row] = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9", title: "Kept across an update" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([nativeProcessKey("codex", "n9")]),
    [terminal],
  );

  assert.equal(row?.hostedTerminal?.runtimeGeneration, "generation-old");
  assert.equal(row?.canOpen, true);
  assert.equal(row?.live, true);
  assert.equal(row ? runningElsewhere(row) : true, false);
});

test("what a row lets a person do follows from where its process is, and from nothing else", () => {
  const hosted = {
    terminalId: "terminal-1",
    runtimeGeneration: "generation-1",
    providerId: "codex",
    workspace: BETA,
    nativeSessionId: "n-hosted",
    processState: "running",
    openedAtMs: NOW,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor;
  const rows = conversations(
    [session({ sessionId: "s-hot", hot: true, lifecycle: "hotIdle" })],
    PROVIDERS,
    [
      nativeChat({ nativeSessionId: "n-hosted", title: "Hosted" }),
      nativeChat({ nativeSessionId: "n-external", title: "External" }),
      nativeChat({ nativeSessionId: "n-stored", title: "Stored" }),
      nativeChat({ nativeSessionId: "n-cold", title: "Cold", resume: "unavailable" }),
    ],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set([nativeProcessKey("codex", "n-external")]),
    [hosted],
  );
  const byTitle = new Map(rows.map((row) => [row.title, row]));
  const expect = (title: string, kind: string, live: boolean, canOpen: boolean, canStop: boolean) => {
    const row = byTitle.get(title);
    assert.ok(row, title);
    assert.equal(row.presence.kind, kind, `${title} presence`);
    assert.deepEqual(
      { live: row.live, canOpen: row.canOpen, canStop: row.canStop },
      { live, canOpen, canStop },
      `${title} facts`,
    );
    assert.equal(row.blocked !== null, !canOpen, `${title} is blocked exactly when it cannot open`);
  };
  expect("Hosted", "hosted", true, true, true);
  expect("External", "external", true, false, false);
  expect("Stored", "stored", false, true, false);
  expect("Cold", "stored", false, false, false);
  const supervised = rows.find((row) => row.session?.sessionId === "s-hot");
  assert.equal(supervised?.presence.kind, "supervised");
  assert.deepEqual(
    { live: supervised?.live, canOpen: supervised?.canOpen, canStop: supervised?.canStop },
    { live: true, canOpen: true, canStop: true },
  );
});

test("a session running in a hosted terminal this window does not view still shows it is working", () => {
  // The service's roster says a model is answering (activeNative). A bare hosted terminal used to be fixed at
  // "ready", so a running conversation read as not running (operator, 2026-08-29).
  const terminal = {
    terminalId: "terminal-busy",
    runtimeGeneration: "generation-old",
    providerId: "claude",
    workspace: BETA,
    nativeSessionId: "n-busy",
    processState: "running",
    openedAtMs: NOW,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor;
  const [row] = conversations(
    [],
    PROVIDERS,
    [],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set([nativeProcessKey("claude", "n-busy")]),
    new Set(),
    [terminal],
  );
  assert.equal(row?.activity, "working", "the roster says it is answering, so the row turns");
});

test("one conversation is one row, even when two generations each hold a terminal for it", () => {
  // The same session hosted in two generations (an old draining one and the new one, or a mirror beside the
  // real process) is one conversation, not two. The sidebar draws it once, on the most recently opened
  // terminal (operator, 2026-08-29: a session showing twice is the bug).
  const process = (terminalId: string, runtimeGeneration: string, openedAtMs: number): TerminalDescriptor => ({
    terminalId,
    runtimeGeneration,
    providerId: "codex",
    workspace: BETA,
    nativeSessionId: "n9",
    processState: "running",
    openedAtMs,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor);
  const rows = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n9", title: "One session" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set(),
    [process("terminal-new", "generation-new", NOW), process("terminal-old", "generation-old", NOW - 1_000)],
  );

  assert.equal(rows.length, 1, "one conversation, one row");
  assert.equal(rows[0]?.title, "One session");
  assert.equal(rows[0]?.hostedTerminal?.terminalId, "terminal-new", "on the most recently opened terminal");
  assert.equal(rows[0]?.canOpen, true);
});

test("a full terminal index becomes sidebar rows within the fifty millisecond p95 budget", () => {
  const count = 256;
  const chats = Array.from({ length: count }, (_, index) => nativeChat({
    providerId: "codex",
    nativeSessionId: `native-${index}`,
    title: `Conversation ${index}`,
  }));
  const terminals = chats.map((chat, index) => ({
    terminalId: `terminal-${index}`,
    runtimeGeneration: "generation-1",
    providerId: chat.providerId,
    workspace: chat.cwd,
    nativeSessionId: chat.nativeSessionId,
    processState: "running",
    openedAtMs: NOW + index,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor));
  const apply = () => conversations(
    [],
    PROVIDERS,
    chats,
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    [],
    new Set(),
    new Set(),
    new Set(),
    terminals,
  );
  assert.equal(apply().length, count);
  const samples = Array.from({ length: 100 }, () => {
    const started = performance.now();
    apply();
    return performance.now() - started;
  }).sort((left, right) => left - right);
  const p95 = samples[Math.floor(samples.length * 0.95)] ?? Number.POSITIVE_INFINITY;
  assert.ok(p95 <= 50, `sidebar row application p95 was ${p95.toFixed(2)} ms`);
});

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

test("a provider placeholder never exposes an internal session identifier", () => {
  const rows = conversations(
    [],
    PROVIDERS,
    [
      nativeChat({ nativeSessionId: "first-8d4a", title: "Untitled" }),
      nativeChat({ nativeSessionId: "second-8980", title: " untitled " }),
    ],
    null,
  );

  assert.deepEqual(new Set(rows.map((row) => row.title)), new Set(["Unnamed conversation"]));
});

test("a Core-owned worktree stays under the project the person selected", () => {
  const worktree = below(ROOT, ".runtrol-worktrees", "chat-01234567");
  const rows = conversations(
    [session({ sessionId: "s1", workspace: worktree, hot: true, lifecycle: "hotIdle" })],
    PROVIDERS,
    [],
    null,
    null,
    new Map(),
    new Map([[workspaceIdentity(worktree), ALPHA]]),
  );
  const grouped = projects([record(ALPHA)], rows, []);

  assert.equal(rows[0]?.workspace, worktree, "actions keep the exact provider working directory");
  assert.equal(rows[0]?.homeWorkspace, ALPHA, "presentation keeps the selected project");
  assert.equal(rows[0]?.title, "Unnamed conversation", "a project name is never repeated as a chat title");
  assert.equal(grouped[0]?.rows[0]?.session?.sessionId, "s1");
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

test("a pinned conversation leads the list even when another was touched more recently", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: BETA }),
      session({ sessionId: "s2", providerId: "codex", nativeSessionId: "n2", workspace: BETA }),
    ],
    PROVIDERS,
    [
      nativeChat({ nativeSessionId: "n1", title: "Older", updatedAt: "2026-08-17T10:00:00Z" }),
      nativeChat({ nativeSessionId: "n2", title: "Newer", updatedAt: "2026-08-17T12:00:00Z" }),
    ],
    null,
    null,
    new Map(),
    new Map(),
    new Set(["chat:codex:n1"]),
  );

  assert.equal(rows[0]?.title, "Older", "the pinned conversation is first though it is the older one");
  assert.equal(rows[0]?.pinned, true, "and it is marked pinned");
  assert.equal(rows.find((row) => row.title === "Newer")?.pinned, false, "the other stays unpinned");
});

test("a local nickname replaces the service's own name, and only for that conversation", () => {
  const rows = conversations(
    [session({ sessionId: "s2", providerId: "codex", nativeSessionId: "n2", workspace: BETA, label: "Service label" })],
    PROVIDERS,
    [
      nativeChat({ nativeSessionId: "n1", title: "Refactor the parser" }),
      nativeChat({ nativeSessionId: "n2", title: "Newer", updatedAt: "2026-08-17T12:00:00Z" }),
    ],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map([["chat:codex:n1", "My renamed chat"]]),
  );

  assert.equal(
    rows.find((row) => row.key === "chat:codex:n1")?.title,
    "My renamed chat",
    "the stored chat shows the local nickname, not the service title",
  );
  assert.equal(
    rows.find((row) => row.key === "chat:codex:n2")?.title,
    "Service label",
    "a conversation with no nickname keeps the name it already had",
  );
});

test("a local nickname wins even over a supervised session's own label", () => {
  const [row] = conversations(
    [session({ sessionId: "s1", providerId: "codex", nativeSessionId: "n1", workspace: BETA, label: "Service label" })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", title: "Stored title" })],
    null,
    null,
    new Map(),
    new Map(),
    new Set(),
    new Map([["chat:codex:n1", "My nickname"]]),
  );

  assert.equal(row?.title, "My nickname", "the operator's nickname names the row, even over the session label");
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

test("a conversation that starts working rises to the top of its list", () => {
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

  // Idle: the two sit in their fixed identity order (ALPHA before BETA).
  assert.deepEqual(idle.map((row) => row.key), working.map((row) => row.key).slice().reverse());
  // s2 answering moves it to the very top: the running conversation leads its project.
  assert.equal(working[0]?.activity, "working");
  assert.equal(working[0]?.workspace, BETA);
});

test("several running conversations keep a fixed order however the bytes arrive", () => {
  // Two conversations answering at once. Recency would reshuffle them on every streamed byte; the list holds a
  // fixed identity order instead so the rows do not thrash under the person (operator, 2026-08-30).
  const oneOrder = conversations(
    [
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: ALPHA }),
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", workspace: BETA }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const otherOrder = conversations(
    [
      session({ sessionId: "s2", hot: true, lifecycle: "hotRunning", workspace: BETA }),
      session({ sessionId: "s1", hot: true, lifecycle: "hotRunning", workspace: ALPHA }),
    ],
    PROVIDERS,
    [],
    null,
  );

  // The order the Runtime happened to list them in does not change where they land.
  assert.deepEqual(oneOrder.map((row) => row.key), otherOrder.map((row) => row.key));
  assert.equal(oneOrder[0]?.workspace, ALPHA);
});

test("a chat the service cannot reopen says so instead of failing on click", () => {
  const rows = conversations(
    [],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", resume: "unavailable", adoptionToken: null })],
    null,
  );

  const [blocked] = rows;
  assert.ok(blocked);
  assert.equal(blocked.canOpen, false);
  assert.ok(blocked.blocked);
  assert.equal(conversationDetail(blocked, NOW), "");
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

test("duplicate missing titles do not leak session identities", () => {
  const rows = conversations(
    [
      session({ sessionId: "s1", workspace: ALPHA }),
      session({ sessionId: "s2", workspace: ALPHA }),
    ],
    PROVIDERS,
    [],
    null,
  );

  assert.deepEqual(new Set(rows.map((row) => row.title)), new Set(["Unnamed conversation"]));
});

test("a conversation line has no detail beside its agent icon and title", () => {
  const [plain] = conversations([session({ sessionId: "s1" })], PROVIDERS, [], null);
  const [named] = conversations([session({ sessionId: "s2", label: "Nightly sweep" })], PROVIDERS, [], null);

  assert.ok(plain);
  assert.ok(named);
  assert.equal(conversationDetail(plain, NOW), "");
  assert.equal(conversationDetail(named, NOW), "");
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
    [nativeChat({ nativeSessionId: "n 1", providerId: "acp-fixture", cwd: below(ROOT, "b") })],
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
  for (const row of rows) assert.equal(row.title, "Unnamed conversation");
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
  const unfiled = loose(rows);
  assert.equal(unfiled.length, 1);
  assert.equal(unfiled[0]?.session?.sessionId, "b");
});

test("a folder that is only whitespace files nowhere", () => {
  const rows = conversations([session({ sessionId: "a", workspace: "   " })], PROVIDERS, [], null);
  const filed = projects([record(ALPHA)], rows, []).flatMap((group) => group.rows);
  assert.equal(filed.length, 0);
  assert.equal(loose(rows).length, 1);
});

test("only an added folder is a heading, so every window shows the same list", () => {
  // A project is a decision, never a discovery: the panel used to invent a heading for every folder with enough
  // conversations, and the operator rejected the wall it produced. The open folder stays explicit; a folder
  // nobody added contributes nothing at all until it is added (operator, 2026-08-26).
  const rows = conversations(spread([ALPHA, BETA, GAMMA]), PROVIDERS, [], null);
  assert.equal(projects([], rows, [ALPHA]).length, 0, "not even the open folder becomes a heading");
  const added = projects([record(ALPHA)], rows, [ALPHA]);
  assert.equal(added.length, 1, "adding it is what makes the heading");
  assert.equal(added[0]?.kind, "created");
  assert.equal(added[0]?.current, true, "the window standing in it is marked, never invented");
  assert.equal(added[0]?.rows.length, 2, "its own conversations file under it");
  assert.equal(loose(rows).length, 0, "an unadded folder's conversations are not on screen at all");
  assert.equal(projects([], [], []).length, 0, "with nothing added, no headings at all");
});

test("adding a folder lists every conversation the services report inside it", () => {
  const rows = conversations(spread([BETA], 3), PROVIDERS, [], null);
  assert.equal(projects([], rows, []).length, 0);
  const groups = projects([record(BETA)], rows, []);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.kind, "created");
  assert.equal(groups[0]?.rows.length, 3, "the folder's conversations file under it at once");
  assert.equal(loose(rows).length, 0);
});

test("pinned projects come first in the order they were added, then the open folder", () => {
  const rows = conversations(spread([ALPHA, BETA, GAMMA]), PROVIDERS, [], null);
  const pinnedGamma = { ...record(GAMMA), pinned: true };
  const pinnedBeta = { ...record(BETA), pinned: true };
  const groups = projects([record(ALPHA), pinnedGamma, pinnedBeta], rows, [ALPHA]);
  assert.deepEqual(groups.map((group) => group.name), ["gamma", "beta", "alpha"]);
  assert.equal(groups[0]?.pinned, true);
  assert.equal(groups[2]?.current, true);
});

test("an added project covering a nested conversation folder draws the one heading", () => {
  const rows = conversations(
    [session({ sessionId: "deep", workspace: below(ALPHA, "packages", "core") })],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA)], rows, []);
  assert.equal(groups.length, 1, "adding is the deliberate act and one row never appears twice");
  assert.equal(groups[0]?.kind, "created");
  assert.equal(groups[0]?.rows.length, 1);
});

const SCRATCH = below(ROOT, "storage", "no-project");

test("a timestamp in seconds, milliseconds or ISO 8601 lands on the same instant", () => {
  // Codex prints seconds, Claude Code prints milliseconds, ACP CLIs print ISO 8601. Measured in the real
  // window: seconds read as milliseconds put every Codex row 56 years in the past ("2952w").
  const at = Date.parse("2026-08-20T10:00:00Z");
  const rows = conversations(
    [],
    PROVIDERS,
    [
      nativeChat({ nativeSessionId: "seconds", providerId: "codex", updatedAt: String(at / 1000) }),
      nativeChat({ nativeSessionId: "millis", providerId: "claude", updatedAt: String(at) }),
      nativeChat({ nativeSessionId: "iso", providerId: "acp-fixture", updatedAt: "2026-08-20T10:00:00Z" }),
    ],
    null,
  );
  for (const row of rows) {
    assert.equal(row.updatedAtMs, at, `${row.native?.nativeSessionId} read wrong`);
  }
});

test("two folders with one name are told apart by their parent, without renaming either", () => {
  // Measured in the real window: two added folders both called new-chat read as one project listed twice
  // unless their parent tells them apart.
  const first = below(ROOT, "2026-08-19", "new-chat");
  const second = below(ROOT, "2026-08-20", "new-chat");
  const rows = conversations(
    [
      session({ sessionId: "a1", workspace: first }),
      session({ sessionId: "b1", workspace: second }),
    ],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(first, "new-chat"), record(second, "new-chat")], rows, []);
  assert.equal(groups.length, 2);
  assert.deepEqual(new Set(groups.map((group) => group.qualifier)), new Set(["2026-08-19", "2026-08-20"]));
  for (const group of groups) {
    assert.equal(group.name, "new-chat", "the name stays the folder's own");
    assert.ok(projectDetail(group).startsWith(`in ${group.qualifier}`));
  }
});

test("a conversation started with no project is loose beneath the headings, never a heading of its own", () => {
  // Condition 3 of the operator's contract: a conversation can exist with no project at all. It runs in the
  // scratch folder, and that folder is an implementation detail: never a heading, never in the row's detail,
  // never its title.
  const rows = conversations(
    [
      session({ sessionId: "filed", workspace: ALPHA }),
      session({ sessionId: "free", workspace: SCRATCH }),
      session({ sessionId: "free-too", workspace: SCRATCH, providerId: "codex" }),
    ],
    PROVIDERS,
    [],
    null,
    SCRATCH,
  );
  const free = rows.filter((row) => row.projectless);
  assert.equal(free.length, 2);
  for (const row of free) {
    assert.equal(row.folder, "", "the scratch folder is not a folder the person chose");
    assert.ok(!row.title.includes("no-project"), `${row.title} leaks the scratch folder`);
    assert.ok(!conversationDetail(row, NOW).includes("no-project"));
  }
  const groups = projects([], rows, []);
  assert.deepEqual(groups.map((group) => group.name), [], "a one-off working directory is not a project");
  assert.equal(loose(rows).length, 2, "only the two with no project of their own, never the filed one");
});

test("without a scratch folder nothing is projectless", () => {
  const rows = conversations([session({ sessionId: "s", workspace: SCRATCH })], PROVIDERS, [], null, null);
  assert.equal(rows[0]?.projectless, false);
  assert.equal(projects([], rows, []).length, 0, "one folder observation does not invent a project");
  assert.equal(loose(rows).length, 0, "it names a folder, so it waits for that folder to be added");
});

test("a created project standing on the open folder draws the one heading", () => {
  const rows = conversations(spread([ALPHA]), PROVIDERS, [], null);
  const groups = projects([record(ALPHA)], rows, [ALPHA]);
  assert.equal(groups.length, 1, "one place, one heading");
  assert.equal(groups[0]?.key.startsWith("project:"), true, "creation is the more deliberate act");
});

test("every conversation row has one exact operational state", () => {
  const [base] = conversations([session({ sessionId: "state" })], PROVIDERS, [], null);
  assert.ok(base);
  assert.equal(conversationStatus({ ...base, activity: "working" }), "Running");
  assert.equal(conversationStatus({ ...base, activity: "ready" }), "Ready");
  assert.equal(conversationStatus({ ...base, activity: "saved" }), "Stopped");
  assert.equal(conversationStatus({ ...base, activity: "needsYou" }), "Needs you");
  assert.equal(conversationStatus({ ...base, activity: "attention" }), "Error");
  assert.equal(conversationStatus({ ...base, activity: "waitingOnQuota" }), "Limit");
  assert.equal(conversationStatus({ ...base, signInNeeded: true }), "Sign in needed");
  assert.equal(conversationStatus({ ...base, canOpen: false }), "Cannot reopen");
});

test("a live conversation without a provider timestamp still says when it is active", () => {
  const [row] = conversations(
    [session({ sessionId: "live-time", hot: true, lifecycle: "hotRunning" })],
    PROVIDERS,
    [],
    null,
  );
  assert.ok(row);
  assert.equal(conversationDetail(row, NOW), "");
});

test("conversation row detail stays empty for every operational state", () => {
  const [base] = conversations([session({ sessionId: "date-only" })], PROVIDERS, [], null);
  assert.ok(base);
  for (const activity of ["needsYou", "attention", "working", "waitingOnQuota", "ready", "saved"] as const) {
    assert.equal(conversationDetail({ ...base, activity }, NOW), "");
  }
});

test("the window's own folder waits to be added like any other", () => {
  // The panel is the machine's, not this window's: opening Runtrol somewhere must not put that somewhere at
  // the top of a list everybody else sees differently (operator, 2026-08-26).
  const rows = conversations([session({ sessionId: "elsewhere", workspace: BETA })], PROVIDERS, [], null);
  assert.deepEqual(projects([], rows, [ALPHA]).map((group) => group.name), []);
  const added = projects([record(ALPHA)], rows, [ALPHA]);
  assert.deepEqual(added.map((group) => group.name), ["alpha"]);
  assert.equal(added[0]?.current, true, "still marked as the folder this window stands in");
  assert.equal(loose(rows).length, 0, "the conversation in the unadded folder is not drawn anywhere");
});

test("a conversation row never repeats the folder its heading already names", () => {
  const rows = conversations(spread([ALPHA]), PROVIDERS, [], null);
  const row = rows[0];
  assert.ok(row);
  assert.ok(!conversationDetail(row, Date.now()).includes(row.folder));
  assert.ok(!conversationDetail(row, Date.now(), true).includes(row.folder));
});

test("an empty heading says what the list holds, never what the folder holds", () => {
  // It was made a moment ago, so the heading stays: one that vanished until a conversation arrived
  // would read as the creation having failed. What it must not do is claim the folder is empty.
  // Zero rows here also happens when a service enumerates only its running sessions, or when the
  // folder sits outside the approved roots, and the heading cannot tell those apart.
  const groups = projects([record(ALPHA)], [], []);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]?.rows.length, 0);
  assert.ok(groups[0]);
  assert.equal(projectDetail(groups[0]), "");
});

test("a conversation in a subfolder files under the project that covers it", () => {
  const rows = conversations(
    [session({ sessionId: "deep", workspace: below(ALPHA, "packages", "core") })],
    PROVIDERS,
    [],
    null,
  );
  const groups = projects([record(ALPHA)], rows, []);
  assert.equal(groups[0]?.rows.length, 1);
  assert.equal(loose(rows).length, 0);
});

test("nested projects file a conversation under the deepest one", () => {
  const sub = below(ALPHA, "packages", "core");
  const rows = conversations(
    [
      session({ sessionId: "inner", workspace: below(sub, "src") }),
      session({ sessionId: "outer", workspace: below(ALPHA, "docs") }),
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
  const unfiled = loose(rows);
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
  for (const row of rows) assert.equal(row.title, "Unnamed conversation");
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

test("the current project comes first even when another project has the newest conversation", () => {
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

test("projects keep the order the person put them in, not the order they were used", () => {
  // A list that reorders itself under the reader is a list they cannot learn. Activity moves nothing.
  const rows = conversations(
    spread([ALPHA, BETA, GAMMA]),
    PROVIDERS,
    [nativeChat({ nativeSessionId: "newest", cwd: GAMMA, updatedAt: "2026-08-17T11:59:00Z" })],
    null,
  );
  const groups = projects([record(BETA), record(GAMMA), record(ALPHA)], rows, [ALPHA]);
  assert.deepEqual(groups.map((group) => group.name), ["beta", "gamma", "alpha"]);
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

test("a heading keeps internal state out of its compact count", () => {
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
  assert.equal(projectDetail(alpha), "3");
});

test("a heading with nothing waiting says only what it holds", () => {
  const rows = conversations(spread([ALPHA, BETA]), PROVIDERS, [], null);
  const beta = projects([record(ALPHA), record(BETA)], rows, []).find((group) => group.name === "beta");
  assert.ok(beta);
  assert.equal(beta.attention, 0);
  assert.match(projectDetail(beta), /^\d+$/u);
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
  assert.equal(loose(rows).length, 0);
});

test("a pin and a nickname survive the moment the service names the conversation", () => {
  // Chosen from the row before the first reply lands, so they are remembered against the session's own key.
  const early = conversations([session({ sessionId: "s1", providerId: "claude", workspace: BETA })], PROVIDERS, [], null);
  const earlyKey = early[0]?.key;
  assert.equal(earlyKey, "session:s1", "a conversation with no service identity yet is keyed by its session");

  const pinnedKeys = new Set([earlyKey ?? ""]);
  const names = new Map([[earlyKey ?? "", "My chat"]]);

  // The first turn arrives and the service announces the identity it knows the conversation by.
  const [later] = conversations(
    [session({ sessionId: "s1", providerId: "claude", nativeSessionId: "n1", workspace: BETA })],
    PROVIDERS,
    [nativeChat({ nativeSessionId: "n1", title: "Service title" })],
    null,
    null,
    new Map(),
    new Map(),
    pinnedKeys,
    names,
  );

  assert.equal(later?.key, "chat:claude:n1", "the row is now keyed by the service's own identity");
  assert.equal(later?.legacyKey, "session:s1", "and it still knows the key it was remembered under");
  assert.equal(later?.pinned, true, "the pin chosen before that moment is still a pin");
  assert.equal(later?.title, "My chat", "and the chosen name is still the name");
});

test("a conversation runtrol just started stands in the list until its service names it", () => {
  const providers = [
    { providerId: "grok", displayName: "Grok", icon: "grok", installation: { state: "usable" } },
  ] as unknown as ProviderLine[];
  const root = "C:/storage/no-project";
  const started = [{
    id: "grok:1",
    providerId: "grok",
    workspace: root,
    title: "New Grok conversation",
    startedAtMs: 1_700_000_000_000,
    runtimeGeneration: "generation-1",
    terminalId: "terminal-1",
  }];
  const terminal = {
    terminalId: "terminal-1",
    runtimeGeneration: "generation-1",
    providerId: "grok",
    workspace: root,
    nativeSessionId: "01a0",
    processState: "running",
    openedAtMs: 1_700_000_000_010,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor;

  // The service has written nothing down yet, which is the whole gap this closes: the person pressed new, a
  // terminal opened, and the list would otherwise still say there are no conversations.
  const fresh = conversations([], providers, [], null, root, new Map(), new Map(), new Set(), new Map(), started);
  assert.deepEqual(fresh.map((row) => [row.title, row.projectless, row.live]), [
    ["New Grok conversation", true, true],
  ]);
  // Nothing may offer to resume, rename or delete it: those need the identity the service has not published.
  assert.equal(fresh[0]?.native, null);
  assert.equal(fresh[0]?.session, null);

  // Once the service describes a running conversation in that folder, that row IS this conversation. Showing
  // both would put one conversation on screen twice.
  const named = conversations(
    [],
    providers,
    [{
      providerId: "grok",
      nativeSessionId: "01a0",
      title: "Simple greeting request",
      cwd: root,
      updatedAt: "1700000030000",
      resume: "available",
      adoptionToken: "t",
      live: true,
    }] as unknown as NativeChatLine[],
    null,
    root,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    started,
    new Set(),
    new Set(),
    new Set(),
    [terminal],
  );
  assert.deepEqual(named.map((row) => row.title), ["Simple greeting request"]);

  // The same judgement, told plainly: which placeholder became which conversation. The tab that was opened for
  // the placeholder reads this to move onto the real conversation, so that the name the service gave reaches
  // the tab and a second click does not open a second tab on it.
  assert.deepEqual([...namedPlaceholders(named, started)], [["grok:1", named[0]!.key]]);
  // A recent conversation from the same provider and folder is still unrelated without this exact terminal.
  const unrelated = named.map((row) => ({ ...row, hostedTerminal: null }));
  assert.deepEqual([...namedPlaceholders(unrelated, started)], []);
  // Nothing to hand over while the service has written nothing: the placeholder stands.
  assert.deepEqual([...namedPlaceholders(fresh.filter((row) => false), started)], []);
  // Another terminal's conversation is not this one, whatever folder or timestamp it carries.
  const elsewhere = named.map((row) => ({
    ...row,
    workspace: "C:/storage/other",
    hostedTerminal: row.hostedTerminal
      ? { ...row.hostedTerminal, terminalId: "terminal-other", workspace: "C:/storage/other" }
      : null,
  }));
  assert.deepEqual([...namedPlaceholders(elsewhere, started)], []);
});

test("two conversations started in separate terminals are promoted independently", () => {
  const root = "C:/work/runtrol";
  const providers = [
    { providerId: "codex", displayName: "Codex", icon: "codex", installation: { state: "usable" } },
  ] as unknown as ProviderLine[];
  const started = [
    {
      id: "codex:first",
      providerId: "codex",
      workspace: root,
      title: "New Codex conversation",
      startedAtMs: 1_700_000_000_000,
      runtimeGeneration: "generation-1",
      terminalId: "terminal-first",
    },
    {
      id: "codex:second",
      providerId: "codex",
      workspace: root,
      title: "New Codex conversation",
      startedAtMs: 1_700_000_000_100,
      runtimeGeneration: "generation-1",
      terminalId: "terminal-second",
    },
  ];
  const terminal = (terminalId: string, nativeSessionId: string, openedAtMs: number) => ({
    terminalId,
    runtimeGeneration: "generation-1",
    providerId: "codex",
    workspace: root,
    nativeSessionId,
    processState: "running",
    openedAtMs,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  }) as TerminalDescriptor;
  const terminals = [
    terminal("terminal-first", "native-first", 1_700_000_000_010),
    terminal("terminal-second", "native-second", 1_700_000_000_110),
  ];
  const rows = conversations(
    [],
    providers,
    [
      nativeChat({
        providerId: "codex",
        nativeSessionId: "native-first",
        title: "Urgent first task",
        cwd: root,
      }),
      nativeChat({
        providerId: "codex",
        nativeSessionId: "native-second",
        title: "Independent second task",
        cwd: root,
      }),
    ],
    null,
    root,
    new Map(),
    new Map(),
    new Set(),
    new Map(),
    started,
    new Set(),
    new Set(),
    new Set(),
    terminals,
  );

  assert.deepEqual(rows.map((row) => row.title).sort(), ["Independent second task", "Urgent first task"]);
  assert.deepEqual([...namedPlaceholders(rows, started)].sort(), [
    ["codex:first", "chat:codex:native-first"],
    ["codex:second", "chat:codex:native-second"],
  ]);
  assert.deepEqual(rows.map((row) => row.hostedTerminal?.terminalId).sort(), [
    "terminal-first",
    "terminal-second",
  ]);
});
