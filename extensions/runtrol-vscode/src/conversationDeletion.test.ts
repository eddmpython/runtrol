import assert from "node:assert/strict";
import test from "node:test";

import { canDelete, conversationDeletion, deletionQuestion } from "./conversationDeletion";
import type { Conversation } from "./conversationList";
import type { NativeChatLine, ProviderCapabilities, SessionLine } from "./runtimeTypes";

const native = {
  providerId: "codex",
  nativeSessionId: "n1",
  cwd: "C:\\work\\alpha",
} as NativeChatLine;

function row(overrides: Partial<Conversation> = {}): Conversation {
  return {
    key: "chat:codex:n1",
    legacyKey: null,
    providerId: "codex",
    serviceName: "Codex",
    serviceIcon: "sparkle",
    title: "Refactor the parser",
    homeWorkspace: "C:\\work\\alpha",
    workspace: "C:\\work\\alpha",
    folder: "alpha",
    projectless: false,
    updatedAtMs: null,
    activity: "saved",
    tool: null,
    signInNeeded: false,
    presence: { kind: "stored", openable: true },
    live: false,
    canStop: false,
    open: false,
    pinned: false,
    session: null,
    native,
    hostedTerminal: null,
    hostedKey: null,
    canOpen: true,
    blocked: null,
    ...overrides,
  };
}

function capabilities(
  nativeSessionDelete: ProviderCapabilities["nativeSessionDelete"],
): ProviderCapabilities {
  return {
    providerId: "codex",
    freshness: "current",
    freshSession: { availability: "available" },
    resume: { availability: "available" },
    structuredEvents: { availability: "available" },
    interrupt: { availability: "available" },
    approvals: { availability: "available" },
    cooling: { availability: "available" },
    nativeSessionCatalogue: { availability: "available" },
    nativeSessionDelete,
  } as ProviderCapabilities;
}

test("a live provider process must be stopped separately before permanent deletion", () => {
  const session = { sessionId: "s1" } as SessionLine;
  const decision = conversationDeletion(
    row({ session, live: true }),
    capabilities({ availability: "available" }),
  );
  assert.equal(decision.kind, "unsupported");
  assert.ok(decision.kind === "unsupported" && decision.why.startsWith("Stop "));
});

test("an unconfirmed former owner cannot be deleted until a current roster resolves it", () => {
  const decision = conversationDeletion(
    row({ presence: { kind: "unconfirmed" }, canOpen: false }),
    capabilities({ availability: "available" }),
  );
  assert.equal(decision.kind, "unsupported");
  assert.ok(decision.kind === "unsupported" && decision.why.includes("must confirm"));
});

test("a supervised pointer without a provider identity is closed rather than called deleted", () => {
  const session = { sessionId: "s1" } as SessionLine;
  assert.equal(conversationDeletion(row({ session, native: null }), null).kind, "unsupported");
});

test("a provider-owned conversation is deleted by the provider only where it says it can", () => {
  assert.deepEqual(
    conversationDeletion(row(), capabilities({ availability: "available", provenance: "officialProtocol" })),
    { kind: "deleteNative", serviceName: "Codex" },
  );
});

test("permanent deletion names the conversation, provider, and absence of a recovery copy", () => {
  const question = deletionQuestion(row(), "Codex");
  assert.equal(question.message, "Permanently delete Refactor the parser from Codex?");
  assert.equal(question.button, "Delete permanently");
  assert.match(question.detail, /no recovery copy/);
});

test("a provider that publishes no deletion is told apart up front, in its own words", () => {
  const decision = conversationDeletion(
    row({ providerId: "claude", serviceName: "Claude Code" }),
    capabilities({ availability: "unsupported", why: "no command or protocol method" }),
  );
  assert.equal(decision.kind, "unsupported");
  assert.ok(decision.kind === "unsupported" && decision.why.includes("no command or protocol method"));
  const unknown = conversationDeletion(row(), capabilities(undefined));
  assert.equal(unknown.kind, "unsupported");
  assert.equal(conversationDeletion(row(), null).kind, "unsupported", "no answer from the Runtime is no permission");
});

test("canDelete is the one truth the row affordance and the click share", () => {
  // An orphan pointer has no exact provider record to remove. Closing it is not permanent conversation deletion.
  const session = { sessionId: "s1", lifecycle: "hotIdle" } as SessionLine;
  assert.equal(canDelete(row({ session, native: null }), null), false);
  assert.equal(canDelete(row({ session, live: true }), capabilities({ availability: "available" })), false);
  // A provider-owned conversation is deletable only where the service says it can be.
  assert.equal(canDelete(row(), capabilities({ availability: "available" })), true);
  assert.equal(canDelete(row(), capabilities({ availability: "unsupported", why: "no method" })), false);
  // A saved row with neither a native identity nor a supervised pointer has nothing to delete.
  assert.equal(canDelete(row({ native: null, session: null }), null), false);
});
