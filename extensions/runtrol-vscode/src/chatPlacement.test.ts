import assert from "node:assert/strict";
import test from "node:test";

import { parallelPlacementRequirement } from "./chatPlacement";

test("a real project leaves every parallel workspace choice to the person", () => {
  assert.equal(parallelPlacementRequirement(1, false), "ask");
  assert.equal(parallelPlacementRequirement(7, false), "ask");
});

test("one service keeps the ordinary path and projectless chat has no worktree choice", () => {
  assert.equal(parallelPlacementRequirement(0, false), "single");
  assert.equal(parallelPlacementRequirement(2, true), "sharedOnly");
});
