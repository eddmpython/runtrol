import assert from "node:assert/strict";
import test from "node:test";

import type { SessionLine } from "./runtimeTypes";
import { orderedSessions, sessionChoices } from "./sessionNavigation";

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
