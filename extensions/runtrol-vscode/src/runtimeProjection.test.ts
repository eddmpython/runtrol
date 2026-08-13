import assert from "node:assert/strict";
import { test } from "node:test";

import { sessionStateLabel } from "./runtimeProjection";

test("public Runtime lifecycle has one stable Studio presentation label", () => {
  assert.equal(sessionStateLabel({ lifecycle: "hotIdle" }), "idle");
  assert.equal(sessionStateLabel({ lifecycle: "hotRunning" }), "busy");
  assert.equal(sessionStateLabel({ lifecycle: "cold" }), "detached");
  assert.equal(sessionStateLabel({ lifecycle: "failed" }), "failed");
});
