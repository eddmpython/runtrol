import assert from "node:assert/strict";
import test from "node:test";

import { WindowRegistryState } from "./windowRegistryState";

const identity = { windowSessionId: "window-1", hostGeneration: "host-1", vscodeVersion: "1.132.1" };

test("a window registers its identity and folders, and publishes every terminal it knows with its command generation", () => {
  const state = new WindowRegistryState(identity, ["C:\\work"]);
  assert.deepEqual(state.register(), {
    windowSessionId: "window-1",
    hostGeneration: "host-1",
    vscodeVersion: "1.132.1",
    workspaceFolders: ["C:\\work"],
  });
  const one = {};
  const two = {};
  state.opened(one, "pwsh");
  state.opened(one, "pwsh again");
  state.opened(two, "cmd");
  assert.deepEqual(state.update(), {
    terminals: [
      { terminalKey: "t1", name: "pwsh", shellIntegration: false },
      { terminalKey: "t2", name: "cmd", shellIntegration: false },
    ],
  });
  assert.equal(state.processResolved(one, 4242), true);
  assert.equal(state.processResolved(one, 4242), false, "the same pid changes nothing");
  assert.equal(state.shellIntegrationChanged(one, "C:\\work"), true);
  const execution = state.executionStarted(one, "claude --resume abc", 2, 1000);
  assert.equal(execution, "e1");
  assert.deepEqual(state.update().terminals[0], {
    terminalKey: "t1",
    name: "pwsh",
    processId: 4242,
    shellIntegration: true,
    cwd: "C:\\work",
    command: { executionId: "e1", commandLine: "claude --resume abc", confidence: 2, startedAtMs: 1000 },
  });
  assert.equal(state.executionEnded(one), true);
  assert.equal(state.executionEnded(one), false, "nothing was running");
  assert.equal(state.executionStarted(one, "codex", 1, 2000), "e2", "command generations climb per window");
  assert.equal(state.closed(two), true);
  assert.equal(state.closed(two), false);
  assert.deepEqual(state.update().terminals.map((terminal) => terminal.terminalKey), ["t1"]);
  assert.equal(state.renamed(one, ""), true, "a terminal opened from the menu has no name until its shell starts");
  assert.equal(state.update().terminals[0]?.name, "");
  assert.equal(state.renamed(one, "powershell"), true);
  assert.equal(state.renamed(one, "powershell"), false);
  state.foldersChanged(["D:\\other"]);
  assert.deepEqual(state.register().workspaceFolders, ["D:\\other"]);
});

test("an unknown terminal is ignored and the published record stays inside the registry's bounds", () => {
  const state = new WindowRegistryState(identity, Array.from({ length: 40 }, (_, index) => `C:\\f${index}`));
  assert.equal(state.register().workspaceFolders.length, 32);
  assert.equal(state.shellIntegrationChanged({}, null), false);
  assert.equal(state.executionStarted({}, "x", 2, 1), null);
  assert.equal(state.executionEnded({}), false);
  for (let index = 0; index < 70; index += 1) state.opened({}, "x".repeat(2000));
  const update = state.update();
  assert.equal(update.terminals.length, 64);
  assert.equal(update.terminals[0]?.name.length, 1024);
  const handle = {};
  state.opened(handle, "t");
  state.executionStarted(handle, "y".repeat(3000), 9, 5);
  const last = state.update().terminals.find((terminal) => terminal.command !== undefined);
  assert.equal(last, undefined, "the terminal past the bound is not published");
});
