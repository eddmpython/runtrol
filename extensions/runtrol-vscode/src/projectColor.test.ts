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
  // Twelve bands over six tab colours: a band names exactly one tab colour (its family), so the pairing is
  // still a function, band to tab. The reverse stopped being one on purpose: the tab narrows to a family of
  // two bands, which is what the six-colour cap on tab icons allows.
  const families = new Map([
    ["hueBlue", "terminal.ansiBlue"],
    ["hueGreen", "terminal.ansiGreen"],
    ["huePurple", "terminal.ansiMagenta"],
    ["hueYellow", "terminal.ansiYellow"],
    ["hueRed", "terminal.ansiRed"],
    ["hueCyan", "terminal.ansiCyan"],
    ["hueOrange", "terminal.ansiYellow"],
    ["hueTeal", "terminal.ansiCyan"],
    ["huePink", "terminal.ansiMagenta"],
    ["hueLime", "terminal.ansiGreen"],
    ["hueBrown", "terminal.ansiRed"],
    ["hueSlate", "terminal.ansiBlue"],
  ]);
  for (let index = 0; index < 60; index += 1) {
    const workspace = `C:/work/project-${index}`;
    const band = rowHueClass(workspace);
    assert.ok(band, `${workspace} took no band`);
    assert.equal(tabColorId(workspace), families.get(band), `${workspace} pairs ${band} with the wrong tab family`);
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
  assert.equal(counts.size, 6, `unreachable tab colours: ${[...counts.keys()].join(", ")}`);
  for (const [colour, count] of counts) {
    assert.ok(count < folders.length * 0.4, `${colour} took ${count} of ${folders.length}`);
  }
  // The band is a class the page's own stylesheet paints. A colour written onto the element instead is dropped
  // by the page's CSP, which is exactly how the band came to be invisible on 2026-08-28.
  const bands = new Set(folders.map((folder) => rowHueClass(folder)));
  assert.equal(bands.size, 12, `unreachable bands: ${[...bands].join(", ")}`);
  for (const folder of folders) assert.match(String(rowHueClass(folder)), /^hue[A-Z]/u);
});
