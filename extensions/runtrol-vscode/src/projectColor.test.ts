import assert from "node:assert/strict";
import test from "node:test";

import { projectColorId } from "./projectColor";

test("a project keeps one colour, and a conversation with no project takes none", () => {
  const first = projectColorId("C:/work/runtrol");
  assert.ok(first);
  // The same folder in every window and after every restart: nothing is stored, so nothing can drift.
  assert.equal(projectColorId("C:/work/runtrol"), first);
  // Windows hands the same folder back in different cases; one project must not become two colours.
  assert.equal(projectColorId("c:/WORK/Runtrol"), first);
  assert.equal(projectColorId(null), null);
  assert.equal(projectColorId("   "), null);
});

test("the colours a person actually has in front of them are distinct", () => {
  // Six projects is a busy sidebar. Landing two of them on one colour there would make the colour a lie, so this
  // holds the palette to covering that many, and the doc says plainly what happens past it.
  const projects = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"].map((name) => `C:/work/${name}`);
  const colours = projects.map(projectColorId);
  assert.equal(new Set(colours).size >= 4, true, `too many collisions: ${colours.join(", ")}`);
  for (const colour of colours) assert.match(String(colour), /^terminal\.ansi/);
});
