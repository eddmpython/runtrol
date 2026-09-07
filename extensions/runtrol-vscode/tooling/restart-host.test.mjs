import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";

test("confirmed restart waits for exact owned tabs, including failed views, and cancellation closes nothing", async () => {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const compiled = await build({
    stdin: { contents: 'export { TerminalTabs } from "./src/terminalTabs"; export { restartExtensionHost } from "./src/connectionActions";',
      resolveDir: root, loader: "ts" },
    bundle: true, platform: "node", format: "cjs", target: "node20", external: ["vscode"], write: false,
    alias: { "@runtrol/runtime-client": path.resolve(root, "../../clients/typescript/src/index.ts") },
  });
  class Emitter {
    listeners = new Set();
    event = (listener) => { this.listeners.add(listener); return { dispose: () => this.listeners.delete(listener) }; };
    fire(value) { for (const listener of this.listeners) listener(value); }
    dispose() { this.listeners.clear(); }
  }
  const closed = new Emitter();
  const terminals = [];
  const disposed = [];
  const detaches = [];
  const errors = [];
  let confirmation;
  let restarts = 0;
  const editor = {
    EventEmitter: Emitter, TerminalLocation: { Editor: 2 }, ProgressLocation: { Window: 10 },
    window: {
      onDidCloseTerminal: closed.event,
      showWarningMessage: async () => confirmation,
      showErrorMessage: async (message) => { errors.push(message); },
      withProgress: (_options, work) => work(),
      createTerminal: (options) => {
        const terminal = { name: options.name, show() {}, dispose() { disposed.push(terminal); } };
        options.pty.onDidChangeName((name) => { terminal.name = name; });
        terminal.close = () => { options.pty.close(); closed.fire(terminal); };
        terminals.push(terminal);
        queueMicrotask(() => options.pty.open());
        return terminal;
      },
    },
    commands: { async executeCommand(command) { assert.equal(command, "workbench.action.restartExtensionHost"); restarts += 1; } },
  };
  const runtime = { async openTerminal({ providerId }) {
    if (providerId === "failed") throw new Error("Fixture open failed");
    return { opened: { terminal: { terminalId: providerId, runtimeGeneration: "fixture" }, viewId: providerId },
      initialScreen: new Uint8Array(), next: () => new Promise(() => undefined), close() {},
      async detach() { detaches.push(providerId); } };
  }, stopTerminal() { throw new Error("Restart must not stop a Runtime terminal"); } };
  const module = { exports: {} };
  const require = createRequire(import.meta.url);
  new Function("module", "exports", "require", compiled.outputFiles[0].text)(module, module.exports,
    (name) => name === "vscode" ? editor : require(name));
  const tabs = new module.exports.TerminalTabs(runtime, () => ({}), () => ({}));
  const first = tabs.showFresh("first", "C:/fixture", "Same name");
  const second = tabs.showFresh("second", "C:/fixture", "Same name");
  const failed = tabs.showFresh("failed", "C:/fixture", "Failed view");
  const unrelated = { name: "Same name", dispose() { throw new Error("Unrelated terminal was closed"); } };
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(errors.length, 1);
  await module.exports.restartExtensionHost(tabs);
  assert.deepEqual(disposed, []);
  assert.equal(restarts, 0);
  confirmation = "Restart extensions";
  const restarting = module.exports.restartExtensionHost(tabs);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(disposed, [first, second, failed]);
  assert.equal(restarts, 0, "dispose requests do not mean the native tabs have closed");
  assert.throws(() => tabs.showFresh("racing", "C:/fixture", "New view"), /Restarting/u);
  closed.fire(unrelated);
  first.close();
  second.close();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(restarts, 0, "a retained failed tab is still owned and must close too");
  failed.close();
  await restarting;
  assert.equal(restarts, 1);
  assert.deepEqual(detaches, ["first", "second"]);
  const delayed = tabs.showFresh("delayed", "C:/fixture", "Delayed close");
  await assert.rejects(module.exports.restartExtensionHost(tabs), /Tabs remain open/u);
  assert.equal(restarts, 1, "a missing close receipt cancels restart instead of discarding the view");
  const retry = tabs.showFresh("retry", "C:/fixture", "Retry remains available");
  delayed.close();
  retry.close();
  tabs.dispose();
  assert.equal(closed.listeners.size, 0);
});
