import assert from "node:assert/strict";
import test from "node:test";

import type { SessionLine } from "./protocol";
import { orderedSessions, sessionChoices } from "./sessionNavigation";

function sessions(count: number): SessionLine[] {
  return Array.from({ length: count }, (_unused, index) => ({
    session: `session-${String(index + 1).padStart(2, "0")}`,
    provider: `provider-${index % 3}`,
    native: null,
    workspace: `C:\\work\\project-${String(index + 1).padStart(2, "0")}`,
    hot: index < 8,
    doing: index < 8 ? "idle" : "cold",
    looks_stuck: index === 11,
  }));
}

test("thirty sessions keep the selection first and every session searchable", () => {
  const listed = sessions(30);
  const selected = listed.at(-1)?.session ?? "";
  const choices = sessionChoices(listed, selected);

  assert.equal(choices.length, 30);
  assert.equal(choices[0]?.session.session, selected);
  assert.equal(choices[0]?.picked, true);
  assert.equal(choices[1]?.session.looks_stuck, true);
  assert.equal(new Set(choices.map((choice) => choice.session.session)).size, 30);
  assert.ok(choices.every((choice) => choice.detail === choice.session.workspace));
});

test("hot and cold groups use deterministic folder ordering", () => {
  const listed = sessions(30).reverse();
  const ordered = orderedSessions(listed, null);

  assert.equal(ordered[0]?.looks_stuck, true);
  assert.deepEqual(
    ordered.filter((session) => session.hot).map((session) => session.session),
    Array.from({ length: 8 }, (_unused, index) => `session-${String(index + 1).padStart(2, "0")}`),
  );
  assert.equal(ordered.at(-1)?.session, "session-30");
});
