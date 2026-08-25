import assert from "node:assert/strict";
import test from "node:test";

import { canDelete, conversationDeletion } from "./conversationDeletion";
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
    live: false,
    open: false,
    pinned: false,
    session: null,
    native,
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

test("a supervised provider-owned conversation is still deleted by the provider", () => {
  const session = { sessionId: "s1" } as SessionLine;
  assert.deepEqual(
    conversationDeletion(row({ session }), capabilities({ availability: "available" })),
    { kind: "deleteNative", serviceName: "Codex" },
  );
});

test("a supervised conversation without a provider identity only forgets its pointer", () => {
  const session = { sessionId: "s1" } as SessionLine;
  assert.deepEqual(conversationDeletion(row({ session, native: null }), null), { kind: "forgetSupervised" });
});

test("a provider-owned conversation is deleted by the provider only where it says it can", () => {
  assert.deepEqual(
    conversationDeletion(row(), capabilities({ availability: "available", provenance: "officialProtocol" })),
    { kind: "deleteNative", serviceName: "Codex" },
  );
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
  // An orphan pointer: runtrol supervises it, but the service no longer lists it. Deletable (forget the
  // pointer), and the row must therefore carry the delete affordance. This is the case that regressed.
  const session = { sessionId: "s1", lifecycle: "hotIdle" } as SessionLine;
  assert.equal(canDelete(row({ session, native: null }), null), true);
  // A provider-owned conversation is deletable only where the service says it can be.
  assert.equal(canDelete(row(), capabilities({ availability: "available" })), true);
  assert.equal(canDelete(row(), capabilities({ availability: "unsupported", why: "no method" })), false);
  // A saved row with neither a native identity nor a supervised pointer has nothing to delete.
  assert.equal(canDelete(row({ native: null, session: null }), null), false);
});
