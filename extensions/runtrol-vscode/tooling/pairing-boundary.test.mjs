import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

test("connection commands share one cold sibling and register account listeners only when needed", async () => {
  const compiled = await build({
    entryPoints: ["connectionSurface", "connectionActions"].map((name) => (
      fileURLToPath(new URL(`../src/${name}.ts`, import.meta.url))
    )),
    outdir: "pairing-memory-output",
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    external: ["vscode", "./connectionActions"],
    write: false,
  });
  const ordinaryRequire = createRequire(import.meta.url);
  const notices = [];
  const requests = [];
  let sibling;
  let loads = 0;
  let panelHtml = "";
  const listeners = new Set();
  let execution;
  const subscribe = (listener) => {
    listeners.add(listener);
    return { dispose: () => listeners.delete(listener) };
  };
  const editor = {
    Task: class { constructor(definition) { this.definition = definition; } },
    ShellExecution: class {},
    TaskScope: { Global: 1 }, TaskRevealKind: { Always: 1 }, TaskPanelKind: { Shared: 1 },
    tasks: {
      onDidEndTask: subscribe, onDidEndTaskProcess: subscribe,
      executeTask: async (task) => { execution = { task }; return execution; },
    },
    ViewColumn: { Active: 1 },
    window: {
      showInformationMessage: async (message) => { notices.push(message); },
      showWarningMessage: async (message) => { notices.push(message); },
      createWebviewPanel: () => {
        const webview = { cspSource: "fixture-origin", html: "" };
        return {
          webview,
          dispose: () => { panelHtml = webview.html; },
          onDidDispose: () => ({ dispose() {} }),
        };
      },
    },
  };
  function load(name) {
    if (name === "vscode") return editor;
    if (name !== "./connectionActions") return ordinaryRequire(name);
    if (!sibling) {
      loads += 1;
      sibling = evaluate("connectionActions");
    }
    return sibling;
  }
  function evaluate(name) {
    const output = compiled.outputFiles.find((file) => file.path.endsWith(`${name}.js`));
    assert.ok(output);
    const module = { exports: {} };
    new Function("module", "exports", "require", output.text)(module, module.exports, load);
    return module.exports;
  }
  const surface = evaluate("connectionSurface");
  assert.equal(loads, 0, "ordinary activation does not load phone UI or QR tables");
  const client = { once: async (request) => {
    requests.push(request.ask);
    if (request.ask === "pairingBegin") return { response: { say: "pairingInvitation", with: {
      pairing_url: "https://example.invalid/pairing-fixture",
      pc_key_fingerprint: "fixture-fingerprint",
      expires_at_ms: 0,
    } } };
    return { response: { say: request.ask, with: [] } };
  } };
  await surface.managePhones(client);
  await surface.reviewPhonePairings(client);
  await surface.pairPhone(client);
  assert.equal(loads, 1);
  assert.equal(listeners.size, 0, "phone UI does not subscribe to account task events");
  assert.deepEqual(requests, ["devices", "pairingProposals", "pairingBegin"]);
  assert.ok(notices.length >= 2);
  const encoded = panelHtml.match(/data:image\/svg\+xml;base64,([A-Za-z0-9+/=]+)/u);
  assert.ok(encoded);
  const svg = Buffer.from(encoded[1], "base64").toString("utf8");
  assert.match(svg, /^<svg /u);
  assert.match(svg, /viewBox="0 0 \d+ \d+"/u);
  assert.ok(encoded[0].length < 32 * 1024, "the pairing image remains bounded without the PNG runtime");
  let refreshed = 0;
  const runtime = { providersUsage: async () => { refreshed += 1; } };
  for (let activation = 0; activation < 2; activation += 1) {
    const context = { subscriptions: [] };
    await surface.runAccountCommand(context, runtime, "fixture", "1", "Sign in", "fixture-command");
    assert.equal(context.subscriptions.length, 1);
    for (const listener of listeners) listener({ execution });
    assert.equal(refreshed, activation + 1);
    for (const subscription of context.subscriptions) subscription.dispose();
    assert.equal(listeners.size, 0, "deactivation releases account listeners and permits reactivation");
  }
  assert.equal(loads, 1, "both connection surfaces reuse the same bounded companion");
});
