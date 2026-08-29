import assert from "node:assert/strict";
import test from "node:test";

import { archivalQuestion, conversationArchival } from "./conversationArchival";
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
    homeWorkspace: native.cwd,
    workspace: native.cwd,
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
  nativeSessionArchive: ProviderCapabilities["nativeSessionArchive"],
): ProviderCapabilities {
  return {
    providerId: "codex",
    freshness: "current",
    nativeSessionArchive,
  } as ProviderCapabilities;
}

test("provider archival stays available while a conversation is supervised", () => {
  const session = { sessionId: "s1" } as SessionLine;
  assert.deepEqual(
    conversationArchival(
      row({ session }),
      capabilities({ availability: "available", provenance: "officialProtocol" }),
    ),
    { kind: "archiveNative", serviceName: "Codex" },
  );
});

test("unsupported archival is explained before attempting it", () => {
  const decision = conversationArchival(
    row({ providerId: "claude", serviceName: "Claude Code" }),
    capabilities({ availability: "unsupported", why: "no official archive surface" }),
  );
  assert.equal(decision.kind, "unsupported");
  assert.ok(decision.kind === "unsupported" && decision.why.includes("no official archive surface"));
});

test("archive confirmation names the conversation and service", () => {
  const question = archivalQuestion(row());
  assert.equal(question.message, 'Archive "Refactor the parser" in Codex?');
  assert.equal(question.button, "Archive in Codex");
});
