import assert from "node:assert/strict";
import test from "node:test";

import { webviewReadyKind } from "./webviewReady";

test("distinguishes startup readiness from an idempotent probe response", () => {
  assert.equal(webviewReadyKind({ type: "webviewReady" }), "startup");
  assert.equal(webviewReadyKind({ type: "webviewReady", probe: true }), "probe");
  assert.equal(webviewReadyKind({ type: "webviewReady", probe: false }), "startup");
});

test("rejects values outside the Webview readiness protocol", () => {
  assert.equal(webviewReadyKind(null), null);
  assert.equal(webviewReadyKind([]), null);
  assert.equal(webviewReadyKind({ type: "selectionRendered", probe: true }), null);
});
