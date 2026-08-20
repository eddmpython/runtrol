import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { validatePublic } from "../src/schema.js";

// Every hello that ever shipped must pass this package's own validator. On 2026-08-20 the
// generated schema started requiring limits fields no installed daemon sent, and every client
// failed at hello against every installed runtime. The corpus is owned by the protocol crate;
// this test makes the TypeScript validator answer for each shipped shape too.
// Compiled tests run from dist/test, so the repo root is four levels up.
const corpusRoot = path.join(
  import.meta.dirname,
  "..", "..", "..", "..",
  "crates", "runtrol-runtime-protocol", "hello_corpus",
);

test("every shipped hello passes the public validator", () => {
  const fixtures = readdirSync(corpusRoot).filter((name) => name.endsWith(".json"));
  assert.ok(fixtures.length > 0, "an empty corpus guards nothing");
  for (const name of fixtures) {
    const body = JSON.parse(readFileSync(path.join(corpusRoot, name), "utf8")) as unknown;
    assert.doesNotThrow(
      () => validatePublic("InitializeResult", body),
      `the shipped hello ${name} no longer validates; a hello field became required or was removed`,
    );
  }
});
