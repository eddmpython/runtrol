import assert from "node:assert/strict";
import test from "node:test";

import type { Conversation } from "./conversationList";
import { applyProjectConversations, planProjectConversations, projectConversationKept, projectConversationQuestion } from "./projectConversations";
import type { ProviderCapabilities } from "./runtimeTypes";

function row(overrides: Partial<Conversation>): Conversation {
  return {
    key: "claude:one",
    title: "one",
    serviceName: "Claude Code",
    serviceIcon: "claude",
    providerId: "claude",
    live: false,
    canStop: false,
    canOpen: true,
    blocked: null,
    pinned: false,
    signInNeeded: false,
    activity: "ready",
    tool: null,
    open: false,
    workspace: "C:\\work\\app",
    homeWorkspace: "C:\\work\\app",
    folder: "app",
    projectless: false,
    updatedAtMs: 0,
    native: {
      providerId: "claude",
      nativeSessionId: "n-1",
      adoptionToken: "token",
    },
    session: null,
    hostedTerminal: null,
    hostedKey: null,
    presence: { kind: "cold" },
    ...overrides,
  } as unknown as Conversation;
}

const DELETES: ProviderCapabilities = {
  nativeSessionDelete: { availability: "available" },
} as unknown as ProviderCapabilities;
const CANNOT: ProviderCapabilities = {
  nativeSessionDelete: { availability: "unavailable", why: "no such command" },
} as unknown as ProviderCapabilities;

test("a project's rows are split into exactly the four fates the confirmation names", () => {
  const rows = [
    row({ key: "a", title: "idle" }),
    row({ key: "b", title: "running here", live: true, canStop: true }),
    row({ key: "c", title: "running outside", live: true, canStop: false }),
    row({ key: "d", title: "kept", providerId: "codex", serviceName: "Codex" }),
    row({ key: "e", title: "idle too" }),
  ];
  const plan = planProjectConversations("delete", rows, (providerId) => (providerId === "claude" ? DELETES : CANNOT));
  assert.deepEqual(plan.eligible.map((entry) => entry.key), ["a", "e"]);
  assert.deepEqual(plan.stoppable.map((entry) => entry.key), ["b"]);
  assert.deepEqual(plan.runningElsewhere.map((entry) => entry.key), ["c"]);
  assert.deepEqual([...plan.unsupported], [["Codex cannot delete this conversation: no such command", 1]]);

  const question = projectConversationQuestion("app", plan);
  assert.ok(question);
  assert.equal(question.message, "Permanently delete 3 of 5 conversations in app?");
  assert.equal(question.idle, "Delete 2 idle");
  assert.equal(question.stopAndApply, "Stop 1 and delete 3");
  assert.match(question.detail, /1 is running here/u);
  assert.match(question.detail, /1 stays: Codex cannot delete this conversation: no such command/u);
  assert.match(question.detail, /1 is running outside Runtrol and is skipped\./u);
});

test("a project with nothing deletable asks no question", () => {
  const rows = [
    row({ key: "a", live: true, canStop: false }),
    row({ key: "b", providerId: "codex", serviceName: "Codex" }),
  ];
  const plan = planProjectConversations("delete", rows, (providerId) => (providerId === "claude" ? DELETES : CANNOT));
  assert.equal(projectConversationQuestion("app", plan), null);
});

test("a running row of a service that cannot delete is kept, never stopped for nothing", () => {
  const rows = [row({ key: "a", live: true, canStop: true, providerId: "codex", serviceName: "Codex" })];
  const plan = planProjectConversations("delete", rows, () => CANNOT);
  assert.equal(plan.stoppable.length, 0);
  assert.deepEqual([...plan.unsupported], [["Codex cannot delete this conversation: no such command", 1]]);
});

test("a fresh owned process is kept for its missing native identity, not blamed on provider capability", () => {
  const saved = row({ key: "saved", title: "saved" });
  const fresh = row({ key: "fresh", title: "Aurora", live: true, canStop: true, native: null });
  const plan = planProjectConversations("delete", [saved, fresh], () => DELETES);
  assert.deepEqual(plan.eligible, [saved]);
  assert.deepEqual(plan.stoppable, []);
  const question = projectConversationQuestion("Aurora", plan);
  assert.ok(question);
  assert.equal(question.message, "Permanently delete 1 of 2 conversations in Aurora?");
  assert.match(question.detail, /Aurora has no exact provider-owned conversation/u);
  assert.doesNotMatch(question.detail, /cannot delete stored conversations/u);
});

const ARCHIVES = { nativeSessionArchive: { availability: "available" } } as ProviderCapabilities;

test("archive uses archive capability and names every retained and stop-required item", () => {
  const rows = [row({ key: "idle" }), row({ key: "running", live: true, canStop: true }),
    row({ key: "foreign", live: true }), row({ key: "unsupported", providerId: "other" }),
    row({ key: "fresh", title: "Fresh", native: null, live: true, canStop: true })];
  const plan = planProjectConversations("archive", rows, (id) => id === "other" ? DELETES : ARCHIVES);
  assert.deepEqual(plan.eligible.map((entry) => entry.key), ["idle"]);
  assert.deepEqual(plan.stoppable.map((entry) => entry.key), ["running"]);
  assert.deepEqual(plan.runningElsewhere.map((entry) => entry.key), ["foreign"]);
  assert.equal([...plan.unsupported.values()].reduce((sum, count) => sum + count, 0), 2);
  const question = projectConversationQuestion("app", plan);
  assert.ok(question);
  assert.equal(question.message, "Archive 2 of 5 conversations in app?");
  assert.equal(question.idle, "Archive 1 idle");
  assert.equal(question.stopAndApply, "Stop 1 and archive 2");
  assert.match(question.detail, /Restore it with that provider/u);
  assert.match(question.detail, /1 is running here/u);
  assert.match(question.detail, /1 is running outside Runtrol/u);
  assert.match(question.detail, /Fresh has no provider-owned conversation/u);
  assert.match(projectConversationKept(plan, false).join("; "), /Stop was not selected/u);
});

test("idle-only archive never stops a running owner or sends foreign and unsupported records", async () => {
  const plan = planProjectConversations("archive", [row({ key: "idle" }),
    row({ key: "running", live: true, canStop: true }), row({ key: "foreign", live: true }),
    row({ key: "fresh", native: null })], () => ARCHIVES);
  const calls: string[] = [];
  const result = await applyProjectConversations(plan, false, {
    async stop(candidate) { calls.push(`stop:${candidate.key}`); },
    async apply(candidate) { calls.push(`archive:${candidate.key}`); },
    report() {},
  });
  assert.deepEqual(calls, ["archive:idle"]);
  assert.deepEqual(result, { intended: 1, completed: 1, refused: [] });
});

test("archive waits for exact stop completion and reports a failure without replay", async () => {
  const plan = planProjectConversations("archive", [row({ key: "running", live: true, canStop: true }),
    row({ key: "later", title: "Later", live: true, canStop: true })], () => ARCHIVES);
  let releaseStop = (): void => { throw new Error("the stop barrier is not initialized"); };
  let markStarted = (): void => { throw new Error("the start barrier is not initialized"); };
  const stopping = new Promise<void>((resolve) => { releaseStop = resolve; });
  const started = new Promise<void>((resolve) => { markStarted = resolve; });
  const calls: string[] = [];
  const operation = applyProjectConversations(plan, true, {
    async stop(candidate) {
      calls.push(`stop:${candidate.key}`);
      if (candidate.key === "running") { markStarted(); await stopping; }
      else throw new Error("exact owner still alive");
    },
    async apply(candidate) { calls.push(`archive:${candidate.key}`); throw new Error("unknown archive outcome"); },
    report() {},
  });
  await started;
  assert.deepEqual(calls, ["stop:running"]);
  releaseStop();
  const result = await operation;
  assert.deepEqual(calls, ["stop:running", "archive:running", "stop:later"]);
  assert.equal(result.completed, 0);
  assert.equal(result.intended, 2);
  assert.deepEqual(result.refused, ["one: unknown archive outcome", "Later: exact owner still alive"]);
});
