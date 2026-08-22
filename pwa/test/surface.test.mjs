import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
const app = await readFile(new URL("../src/app.js", import.meta.url), "utf8");
const worker = await readFile(new URL("../service-worker.js", import.meta.url), "utf8");
const styles = await readFile(new URL("../styles.css", import.meta.url), "utf8");
const signals = await readFile(new URL("../src/missionSignals.js", import.meta.url), "utf8");

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

test("content-free attention resolves through bounded Mission Flight Signals", () => {
  assert.match(html, /id="mission-signal-count"[^>]*hidden/u);
  assert.match(html, /id="mission-flight-signal"[^>]*role="status"[^>]*hidden/u);
  assert.match(app, /listMissionFlightSignals\(state\.connection\.missionSignalCursor\)/u);
  assert.match(app, /missionFlightDestination\(state\.flightSignals, state\.sessions\)/u);
  assert.match(signals, /const MAX_SIGNALS = 64;/u);
  assert.match(signals, /row\.waiting_on === "person"/u);
  assert.doesNotMatch(worker, /mission(?:Id|_id|:)/u);
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
