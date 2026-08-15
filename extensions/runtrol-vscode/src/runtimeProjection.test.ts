import assert from "node:assert/strict";
import { test } from "node:test";

import { sessionStateLabel } from "./runtimeProjection";

test("public Runtime lifecycle has one stable Studio presentation label", () => {
  assert.equal(sessionStateLabel({ lifecycle: "hotIdle" }), "Ready");
  assert.equal(sessionStateLabel({ lifecycle: "hotRunning" }), "Working");
  assert.equal(sessionStateLabel({ lifecycle: "cold" }), "Saved");
  assert.equal(sessionStateLabel({ lifecycle: "failed" }), "Needs attention");
});
