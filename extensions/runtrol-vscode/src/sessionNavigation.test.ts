import assert from "node:assert/strict";
import test from "node:test";

import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { chatServices, orderedSessions, sessionChoices } from "./sessionNavigation";

const PROVIDERS: ProviderLine[] = [
  {
    providerId: "claude",
    displayName: "Claude Code",
    installation: { state: "usable", version: "1.0.0" },
  },
  {
    providerId: "codex",
    displayName: "Codex CLI",
    installation: { state: "unavailable", why: "not installed" },
  },
];

function sessions(count: number): SessionLine[] {
  return Array.from({ length: count }, (_unused, index) => ({
    sessionId: `session-${String(index + 1).padStart(2, "0")}`,
    providerId: `provider-${index % 3}`,
    nativeSessionId: null,
    label: null,
    workspace: `C:\\work\\project-${String(index + 1).padStart(2, "0")}`,
    hot: index < 8,
    lifecycle: index < 8 ? "hotIdle" : "cold",
    looksStuck: index === 11,
    sessionGeneration: 1,
  }));
}

test("thirty sessions keep the selection first and every session searchable", () => {
  const listed = sessions(30);
  const selected = listed.at(-1)?.sessionId ?? "";
  const choices = sessionChoices(listed, selected);

  assert.equal(choices.length, 30);
  assert.equal(choices[0]?.session.sessionId, selected);
  assert.equal(choices[0]?.picked, true);
  assert.equal(choices[1]?.session.looksStuck, true);
  assert.equal(new Set(choices.map((choice) => choice.session.sessionId)).size, 30);
  assert.ok(choices.every((choice) => choice.detail.includes(choice.session.workspace)));
});

test("hot and cold groups use deterministic folder ordering", () => {
  const listed = sessions(30).reverse();
  const ordered = orderedSessions(listed, null);

  assert.equal(ordered[0]?.looksStuck, true);
  assert.deepEqual(
    ordered.filter((session) => session.hot).map((session) => session.sessionId),
    Array.from({ length: 8 }, (_unused, index) => `session-${String(index + 1).padStart(2, "0")}`),
  );
  assert.equal(ordered.at(-1)?.sessionId, "session-30");
});

test("chat services preserve discovery order and include services without chats", () => {
  const services = chatServices([chatSession("one", "claude", true)], PROVIDERS, "one");
  assert.deepEqual(services.map((service) => service.displayName), ["Claude Code", "Codex CLI"]);
  assert.equal(services[0]?.selected, true);
  assert.equal(services[1]?.sessions.length, 0);
});

test("provider-owned chats stay under their service and unknown providers remain reachable", () => {
  const listed = [
    chatSession("cold", "claude"),
    chatSession("hot", "claude", true),
    chatSession("other", "future-provider", true),
  ];
  const services = chatServices(listed, PROVIDERS, "hot");
  assert.deepEqual(services.map((service) => service.providerId), ["claude", "codex", "future-provider"]);
  assert.deepEqual(services[0]?.sessions.map((chat) => chat.sessionId), ["hot", "cold"]);
  assert.equal(services[2]?.provider, null);
  assert.equal(services[2]?.displayName, "Future Provider");
});

test("official existing chats join their service without duplicating managed chats", () => {
  const managed = chatSession("managed", "claude");
  const native: NativeChatLine[] = [
    nativeChat("native-managed", "claude", "Already managed", "2026-08-15T10:00:00Z"),
    nativeChat("older", "claude", "Older", "2026-08-14T10:00:00Z"),
    nativeChat("newer", "claude", "Newer", "2026-08-16T10:00:00Z"),
    nativeChat("future", "future-provider", "Future", "2026-08-16T11:00:00Z"),
    { ...nativeChat("adopted", "claude", "Adopted", "2026-08-16T12:00:00Z"), alreadyManagedAs: "other" },
  ];
  const services = chatServices([managed], PROVIDERS, null, native);

  assert.deepEqual(services[0]?.nativeChats.map((chat) => chat.nativeSessionId), ["newer", "older"]);
  assert.deepEqual(services.map((service) => service.providerId), ["claude", "codex", "future-provider"]);
  assert.equal(services[2]?.nativeChats[0]?.title, "Future");
});

function chatSession(sessionId: string, providerId: string, hot = false): SessionLine {
  return {
    sessionId,
    providerId,
    nativeSessionId: `native-${sessionId}`,
    label: null,
    workspace: `C:\\work\\${sessionId}`,
    hot,
    lifecycle: hot ? "hotIdle" : "cold",
    looksStuck: false,
    sessionGeneration: 1,
  };
}

function nativeChat(
  nativeSessionId: string,
  providerId: string,
  title: string,
  updatedAt: string,
): NativeChatLine {
  return {
    providerId,
    nativeSessionId,
    cwd: `C:\\work\\${nativeSessionId}`,
    additionalDirectories: [],
    title,
    updatedAt,
    resume: "available",
    adoptionToken: `token-${nativeSessionId}`,
  };
}
