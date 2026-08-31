import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
const app = await readFile(new URL("../src/app.js", import.meta.url), "utf8");
const relay = await readFile(new URL("../src/relay.js", import.meta.url), "utf8");
const worker = await readFile(new URL("../service-worker.js", import.meta.url), "utf8");
const styles = await readFile(new URL("../styles.css", import.meta.url), "utf8");

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

test("the session surface exposes one bounded attention entry point", () => {
  assert.match(html, /id="next-attention"[^>]*hidden/u);
  assert.match(html, /id="attention-count">0</u);
  assert.match(app, /nextAttentionSession\(state\.sessions, state\.selected\?\.session/u);
  assert.match(app, /session\.waiting_on === "quota"/u);
});

test("the phone carries no trace of the Mission surface this product removed", () => {
  // Deleted from the extension and the Core, and it lived here too: markup, styles, two modules and the
  // wiring between them. Code that calls a surface which no longer exists fails the moment it is pressed, and
  // this is what stops it coming back with the next copied file.
  for (const source of [html, app, relay, worker, styles]) assert.doesNotMatch(source, /mission/iu);
});

test("hidden phone surfaces cannot occupy layout space", () => {
  assert.match(styles, /\[hidden\] \{ display: none !important; \}/u);
  assert.match(styles, /\.session-detail:not\(\[hidden\]\).*z-index: 5/u);
});

test("a generic push carries only a content-free focus intent", () => {
  assert.match(worker, /showNotification\("Runtrol needs attention"/u);
  assert.match(worker, /postMessage\(\{ kind: "runtrolAttention" \}\)/u);
  assert.match(worker, /openWindow\("\.\/\?attention=1"\)/u);
  assert.doesNotMatch(worker, /event\.data/u);
  assert.doesNotMatch(worker, /session(?:Id|_id|:)\s*/u);
});

test("usage is drawn from the pushed session index, icon and progress, never from a clock", () => {
  assert.match(html, /id="usage-strip"[^>]*hidden/u);
  assert.match(app, /state\.usage = Array\.isArray\(listing\.usage\) \? listing\.usage : \[\];/u);
  assert.match(app, /await client\.beginSessionWatch\(\);/u);
  assert.match(app, /renderUsage\(\);/u);
  assert.doesNotMatch(app, /setInterval\(/u);
  assert.match(styles, /\.usage-row \.usage-meter > span/u);
});
