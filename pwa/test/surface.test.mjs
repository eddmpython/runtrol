import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
const app = await readFile(new URL("../src/app.js", import.meta.url), "utf8");

test("the shipped CSP permits its own runtime presentation contract", () => {
  assert.match(html, /connect-src 'self' https: wss:;/u);
});

test("pairing reports readiness without exposing connected-device actions", () => {
  const start = app.indexOf("async function showPairing() {");
  const end = app.indexOf("\nfunction showUnpaired()", start);
  assert.ok(start >= 0 && end > start, "the pairing surface implementation is present");
  const source = app.slice(start, end);
  assert.match(source, /forget\.hidden = true;/u);
  assert.match(source, /notifications\.hidden = true;/u);
  assert.match(source, /setStatus\("Ready to pair", "connecting"\);/u);
});
