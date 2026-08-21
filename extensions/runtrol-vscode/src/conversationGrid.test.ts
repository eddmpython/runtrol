import assert from "node:assert/strict";
import test from "node:test";

import { MAX_GRID_CELLS, gridCells, gridColumns, gridLayout } from "./conversationGrid";

test("the grid is as square as the count allows, taller on the left", () => {
  assert.deepEqual(gridColumns(1), [1]);
  assert.deepEqual(gridColumns(2), [1, 1]);
  assert.deepEqual(gridColumns(3), [2, 1]);
  assert.deepEqual(gridColumns(4), [2, 2]);
  assert.deepEqual(gridColumns(5), [2, 2, 1]);
  assert.deepEqual(gridColumns(6), [2, 2, 2]);
  assert.deepEqual(gridColumns(7), [3, 2, 2]);
  assert.deepEqual(gridColumns(9), [3, 3, 3]);
});

test("nine is the most the editor addresses, and a larger count is cut there", () => {
  assert.deepEqual(gridColumns(12), gridColumns(MAX_GRID_CELLS));
  assert.equal(gridCells(12).length, MAX_GRID_CELLS);
  assert.deepEqual(gridColumns(0), []);
  assert.deepEqual(gridCells(0), []);
});

test("the layout is columns side by side, each stacked, in the editor's own shape", () => {
  assert.deepEqual(gridLayout(3), {
    orientation: 0,
    groups: [{ groups: [{}, {}] }, { groups: [{}] }],
  });
  assert.deepEqual(gridCells(3), [1, 2, 3]);
});
