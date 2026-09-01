import assert from "node:assert/strict";
import test from "node:test";

import { PROJECT_ACCENTS, projectAccentColor } from "./projectColor";

test("one project has one exact accent in every Windows path spelling", () => {
  const first = projectAccentColor("C:/work/runtrol");
  assert.match(first, /^#[0-9a-f]{6}$/u);
  assert.equal(projectAccentColor("C:/work/runtrol"), first);
  assert.equal(projectAccentColor("c:/WORK/Runtrol"), first);
  assert.equal(projectAccentColor(null), PROJECT_ACCENTS[0]);
  assert.equal(projectAccentColor("   "), PROJECT_ACCENTS[0]);
});

test("the fixed provider-glyph palette is distinct and fully reachable", () => {
  assert.equal(new Set(PROJECT_ACCENTS).size, PROJECT_ACCENTS.length);
  const folders = Array.from({ length: 400 }, (_, index) => `C:/work/project-${index}`);
  const reached = new Set(folders.map((folder) => projectAccentColor(folder)));
  assert.deepEqual(reached, new Set(PROJECT_ACCENTS));
});
