import assert from "node:assert/strict";
import test from "node:test";

import { readStartDefaults, rememberStartDefault } from "./startDefaults";

test("reads only rows that still parse and drops the rest silently", () => {
  const stored = {
    good: { providerId: "claude", model: "sonnet", effort: null, permission: "plan", atMs: 5 },
    noProvider: { model: "x", atMs: 5 },
    noStamp: { providerId: "codex" },
    junk: "not a row",
  };
  const defaults = readStartDefaults(stored);
  assert.deepEqual(Object.keys(defaults), ["good"]);
  assert.equal(defaults.good?.model, "sonnet");
  assert.equal(defaults.good?.permission, "plan");
  assert.deepEqual(readStartDefaults(null), {});
  assert.deepEqual(readStartDefaults([1, 2]), {});
});

test("remembering replaces the same project and prunes the least recently used past the bound", () => {
  let defaults = readStartDefaults({});
  for (let index = 0; index < 70; index += 1) {
    defaults = rememberStartDefault(
      defaults,
      `project-${index}`,
      { providerId: "claude", model: null, effort: null, permission: null },
      index,
    );
  }
  assert.equal(Object.keys(defaults).length, 64);
  assert.equal(defaults["project-0"], undefined, "the oldest fell off");
  assert.ok(defaults["project-69"], "the newest stays");
  const replaced = rememberStartDefault(
    defaults,
    "project-69",
    { providerId: "codex", model: "gpt-5", effort: "high", permission: null },
    100,
  );
  assert.equal(Object.keys(replaced).length, 64, "replacement is not growth");
  assert.equal(replaced["project-69"]?.providerId, "codex");
});
