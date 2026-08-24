import assert from "node:assert/strict";
import test from "node:test";

import { composerContextLabel } from "./composerContext";

test("composer context labels make every message destination explicit", () => {
  assert.equal(composerContextLabel("Project", "runtrol"), "Project: runtrol");
  assert.equal(composerContextLabel("Branch", "main"), "Branch: main");
  assert.equal(composerContextLabel("Agent", "Codex"), "Agent: Codex");
  assert.equal(composerContextLabel("Project", ""), "");
});
