import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { sessionContext, sessionTitle, uniqueSessionTitle } from "./sessionDisplay";

const PROVIDERS: ProviderLine[] = [
  {
    providerId: "claude",
    displayName: "Claude Code",
    installation: { state: "usable", version: "1.0.0" },
  },
];

function session(overrides: Partial<SessionLine> = {}): SessionLine {
  return {
    sessionId: "019fcafe-0000-7000-8000-123456abcdef",
    providerId: "claude",
    nativeSessionId: "provider-session-123456",
    label: null,
    workspace: "C:\\work\\runtrol",
    hot: true,
    lifecycle: "hotIdle",
    looksStuck: false,
    sessionGeneration: 1,
    ...overrides,
  };
}

test("the default name combines the project and discovered provider name", () => {
  assert.equal(uniqueSessionTitle(session(), [session()], PROVIDERS), "runtrol · Claude Code");
  assert.equal(sessionContext(session(), PROVIDERS), "runtrol · Claude Code");
});

test("an operator name becomes primary without hiding project context", () => {
  const named = session({ label: "Fix update rollback" });
  assert.equal(sessionTitle(named), "Fix update rollback");
  assert.equal(sessionContext(named, PROVIDERS), "runtrol · Claude Code");
});

test("duplicate fallback names gain a short stable discriminator only when needed", () => {
  const first = session();
  const second = session({
    sessionId: "019fcafe-0000-7000-8000-fedcba654321",
    nativeSessionId: "provider-session-654321",
  });
  assert.equal(uniqueSessionTitle(first, [first], PROVIDERS), "runtrol · Claude Code");
  assert.equal(uniqueSessionTitle(first, [first, second], PROVIDERS), "runtrol · Claude Code · #123456");
  assert.equal(uniqueSessionTitle(second, [first, second], PROVIDERS), "runtrol · Claude Code · #654321");
});
