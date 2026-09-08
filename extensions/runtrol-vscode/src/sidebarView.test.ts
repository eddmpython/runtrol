import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import test from "node:test";
import { runInNewContext } from "node:vm";
import type { SidebarView as View } from "./sidebarView";
import type { SidebarModel } from "./sidebarPage";

const requireFromTest = createRequire(__filename);
const { buildSync } = requireFromTest("esbuild") as typeof import("esbuild");
const extensionRoot = resolve(__dirname, "..");
const source = resolve(extensionRoot, "src/sidebarView.ts");
const output = buildSync({ entryPoints: [source], bundle: true, platform: "node", format: "cjs", target: "node20",
  write: false, external: ["vscode"],
  alias: { "@runtrol/runtime-client": resolve(extensionRoot, "../../clients/typescript/src/index.ts") } });
const loaded: { exports: { SidebarView?: typeof View } } = { exports: {} };
runInNewContext(output.outputFiles[0].text, {
  module: loaded, exports: loaded.exports,
  require: (id: string) => id === "vscode" ? { Uri: { joinPath: () => ({}) } } : requireFromTest(id),
  Buffer, process, console, TextEncoder, TextDecoder, setTimeout, clearTimeout,
}, { filename: source });
assert.ok(loaded.exports.SidebarView);
const SidebarView = loaded.exports.SidebarView;

test("sidebar paints coalesce before ready and recreate a full baseline without losing retained visibility", async () => {
  let model: SidebarModel = { projects: [], loose: [], usage: [], notices: [], serviceChoice: null, firstRun: false, version: "test" };
  const sidebar = Object.assign(Object.create(SidebarView.prototype), {
    context: { extensionUri: {} }, state: { usage: [] }, subscriptions: [],
    documentNonce: null, lastRendered: null, pageReady: false, usageReset: null,
    updateContext() {}, updateBadge() {}, flushReveal() {}, buildModel: () => model,
    report(error: unknown) { throw error; },
  }) as View;
  const sent: Array<{ type: string; body: string }> = [];
  let delivered = true;
  let receive!: (message: unknown) => void;
  let visibility!: () => void;
  let dispose!: () => void;
  const view = {
    visible: true, title: undefined,
    webview: { options: {}, html: "", cspSource: "test:",
      onDidReceiveMessage(listener: typeof receive) { receive = listener; },
      async postMessage(message: { type: string; body: string }) { sent.push(message); return delivered; },
    },
    onDidDispose(listener: () => void) { dispose = listener; },
    onDidChangeVisibility(listener: () => void) { visibility = listener; },
  };
  sidebar.resolveWebviewView(view as unknown as Parameters<View["resolveWebviewView"]>[0]);
  assert.ok(view.webview.html.includes("<!DOCTYPE html>"));
  model = { ...model, notices: [{ tone: "info", text: "Latest before ready", command: null, label: null }] };
  sidebar.setStaleWindow(null);
  assert.equal(sent.length, 0);
  receive({ type: "ready" });
  await Promise.resolve();
  assert.equal(sent.length, 1);
  assert.ok(sent[0].body.includes("Latest before ready"));
  assert.ok(!sent[0].body.includes("data-retain"));
  sidebar.setStaleWindow(null);
  assert.equal(sent.length, 1, "unchanged ready projection sends nothing");
  view.visible = false;
  visibility();
  model = { ...model, notices: [{ tone: "info", text: "While hidden", command: null, label: null }] };
  sidebar.setStaleWindow(null);
  assert.equal(sent.length, 1);
  view.visible = true;
  visibility();
  assert.equal(sent.length, 2, "retained document needs no new ready message");
  assert.ok(sent[1].body.includes("data-retain"));
  receive({ type: "ready" });
  assert.equal(sent.length, 3);
  assert.ok(!sent[2].body.includes("data-retain"), "a reloaded document receives a full baseline");
  delivered = false;
  model = { ...model, notices: [{ tone: "info", text: "Delivery retry", command: null, label: null }] };
  sidebar.setStaleWindow(null);
  await Promise.resolve();
  delivered = true;
  visibility();
  assert.equal(sent.length, 5);
  assert.ok(!sent[4].body.includes("data-retain"), "failed delivery retries a full baseline without another ready event");
  dispose();
  receive({ type: "ready" });
  assert.equal(sent.length, 5, "retired document callbacks cannot publish");
  sidebar.resolveWebviewView(view as unknown as Parameters<View["resolveWebviewView"]>[0]);
  assert.ok(view.webview.html.includes("Delivery retry"));
  sidebar.dispose();
});
