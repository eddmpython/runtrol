import assert from "node:assert/strict";
import test from "node:test";

import type { Conversation } from "./conversationList";
import { planProjectDeletion, projectDeletionQuestion } from "./projectDeletion";
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
  const plan = planProjectDeletion(rows, (providerId) => (providerId === "claude" ? DELETES : CANNOT));
  assert.deepEqual(plan.deletable.map((entry) => entry.key), ["a", "e"]);
  assert.deepEqual(plan.stoppable.map((entry) => entry.key), ["b"]);
  assert.deepEqual(plan.runningElsewhere.map((entry) => entry.key), ["c"]);
  assert.deepEqual([...plan.undeletable], [["Codex", 1]]);

  const question = projectDeletionQuestion("app", plan);
  assert.ok(question);
  assert.equal(question.message, "Permanently delete 3 of 5 conversations in app?");
  assert.equal(question.deleteIdle, "Delete 2 idle");
  assert.equal(question.stopAndDelete, "Stop 1 and delete 3");
  assert.match(question.detail, /1 is running here/u);
  assert.match(question.detail, /1 stays: Codex cannot delete stored conversations\./u);
  assert.match(question.detail, /1 is running outside Runtrol and is skipped\./u);
});

test("a project with nothing deletable asks no question", () => {
  const rows = [
    row({ key: "a", live: true, canStop: false }),
    row({ key: "b", providerId: "codex", serviceName: "Codex" }),
  ];
  const plan = planProjectDeletion(rows, (providerId) => (providerId === "claude" ? DELETES : CANNOT));
  assert.equal(projectDeletionQuestion("app", plan), null);
});

test("a running row of a service that cannot delete is kept, never stopped for nothing", () => {
  const rows = [row({ key: "a", live: true, canStop: true, providerId: "codex", serviceName: "Codex" })];
  const plan = planProjectDeletion(rows, () => CANNOT);
  assert.equal(plan.stoppable.length, 0);
  assert.deepEqual([...plan.undeletable], [["Codex", 1]]);
});
