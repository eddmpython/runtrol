import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import test from "node:test";
import { runInNewContext } from "node:vm";
import type { TerminalTabs as Tabs } from "./terminalTabs";
import { terminalIdentity } from "./runtimeTerminal";

// Load the actual adapter without opening an Extension Host, as windowRegistry.test.ts does.
const requireFromTest = createRequire(__filename);
const { buildSync } = requireFromTest("esbuild") as typeof import("esbuild");
const extensionRoot = resolve(__dirname, "..");
const source = resolve(extensionRoot, "src/terminalTabs.ts");
const output = buildSync({ entryPoints: [source], bundle: true, platform: "node", format: "cjs", target: "node20",
  write: false, external: ["vscode"],
  alias: { "@runtrol/runtime-client": resolve(extensionRoot, "../../clients/typescript/src/index.ts") } });
const loaded: { exports: { TerminalTabs?: typeof Tabs } } = { exports: {} };
runInNewContext(output.outputFiles[0].text, {
  module: loaded, exports: loaded.exports, require: (id: string) => id === "vscode" ? {} : requireFromTest(id),
  Buffer, process, console, TextEncoder, TextDecoder, setTimeout, clearTimeout, setInterval, clearInterval,
}, { filename: source });
assert.ok(loaded.exports.TerminalTabs);
const TerminalTabs = loaded.exports.TerminalTabs;

test("exact journey input keeps a recovering host but refuses retired and other-generation tabs", async () => {
  const terminal = {};
  const identity = terminalIdentity("generation", "terminal");
  let release!: () => void;
  const recovering = new Promise<void>((resolve) => { release = resolve; });
  const timing = { receivedAtMs: 1, dispatchedAtMs: 2, acknowledgedAtMs: 3 };
  let admitted = 0;
  const host = { descriptor: () => null, async handleMeasuredInput() { admitted++; await recovering; return timing; } };
  // Populate only the existing tab ownership records. A real recovery intentionally has no current descriptor.
  const tabs = Object.assign(Object.create(TerminalTabs.prototype), {
    hosts: new Map([[terminal, host]]), journeyTargetByTab: new Map([[terminal, identity]]),
    journeyEnds: new Map<string, string>(),
  }) as Tabs;
  const pending = tabs.writeDirectJourneyInput("generation", "terminal", "x");
  assert.equal(admitted, 1, "the input reaches the existing host's recovery-aware queue");
  release();
  assert.equal(await pending, timing);
  assert.throws(() => tabs.writeDirectJourneyInput("other-generation", "terminal", "x"), /not open/);
  Object.assign(tabs, {
    hosts: new Map([[terminal, null]]), journeyEnds: new Map([[identity, "recovery failed"]]),
  });
  assert.throws(() => tabs.writeDirectJourneyInput("generation", "terminal", "x"), /recovery failed/);
  assert.equal(admitted, 1, "the retained identity never revives a retired host");
});
