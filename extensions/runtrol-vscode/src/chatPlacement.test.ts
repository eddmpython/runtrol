import assert from "node:assert/strict";
import test from "node:test";

import { multiProviderPlacement } from "./chatPlacement";

test("multiple services never share a selected project checkout", () => {
  assert.equal(multiProviderPlacement(1, false), "isolated");
  assert.equal(multiProviderPlacement(7, false), "isolated");
});

test("one service keeps the ordinary collision decision and no-project chat needs no Git worktree", () => {
  assert.equal(multiProviderPlacement(0, false), null);
  assert.equal(multiProviderPlacement(2, true), "shared");
});
