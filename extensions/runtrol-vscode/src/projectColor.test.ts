import assert from "node:assert/strict";
import test from "node:test";

import { rowHueClass, tabColorId } from "./projectColor";

test("a project keeps one colour, and a conversation with no project takes none", () => {
  const first = tabColorId("C:/work/runtrol");
  assert.ok(first);
  // The same folder in every window and after every restart: nothing is stored, so nothing can drift.
  assert.equal(tabColorId("C:/work/runtrol"), first);
  // Windows hands the same folder back in different cases; one project must not become two colours.
  assert.equal(tabColorId("c:/WORK/Runtrol"), first);
  assert.equal(tabColorId(null), null);
  assert.equal(tabColorId("   "), null);
  assert.equal(rowHueClass(null), null);
  assert.equal(rowHueClass("   "), null);
});

test("the tab and the sidebar band name the same hue for one project", () => {
  // The whole point of the colour is that a tab and the heading it came from are recognised as one project.
  // They are read by two surfaces with two vocabularies, so the only thing holding them together is that the
  // lists agree hue for hue. A reordering of either list is caught here rather than by a reader's eye.
  const hues = new Map([
    ["terminal.ansiBlue", "hueBlue"],
    ["terminal.ansiGreen", "hueGreen"],
    ["terminal.ansiMagenta", "huePurple"],
    ["terminal.ansiYellow", "hueYellow"],
    ["terminal.ansiRed", "hueRed"],
  ]);
  for (const name of ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"]) {
    const workspace = `C:/work/${name}`;
    const tab = tabColorId(workspace);
    assert.ok(tab, `${workspace} took no tab colour`);
    assert.equal(rowHueClass(workspace), hues.get(tab), `${workspace} pairs ${tab} with the wrong band`);
  }
});

test("the palette spreads, so a sidebar of projects is not one colour", () => {
  // Five names picked by hand tell you nothing: five draws from five slots collide by arithmetic, not by a
  // fault in the hash. What is worth holding is that every slot is reachable and none of them swallows the
  // list, which is what makes the colour narrow a reader's guess at all.
  const folders = Array.from({ length: 200 }, (_, index) => `C:/work/project-${index}`);
  const counts = new Map<string, number>();
  for (const folder of folders) {
    const colour = tabColorId(folder);
    assert.ok(colour, `${folder} took no colour`);
    assert.match(colour, /^terminal\.ansi/u);
    counts.set(colour, (counts.get(colour) ?? 0) + 1);
  }
  assert.equal(counts.size, 5, `unreachable slots: ${[...counts.keys()].join(", ")}`);
  for (const [colour, count] of counts) {
    assert.ok(count < folders.length * 0.4, `${colour} took ${count} of ${folders.length}`);
  }
  // The band is a class the page's own stylesheet paints. A colour written onto the element instead is dropped
  // by the page's CSP, which is exactly how the band came to be invisible on 2026-08-28.
  for (const folder of folders) assert.match(String(rowHueClass(folder)), /^hue[A-Z]/u);
});
