import assert from "node:assert/strict";
import test from "node:test";
import type { TerminalDescriptor } from "@runtrol/runtime-client";
import { canOpenInputView } from "./observedInput";

test("only a current observed owner input capability offers an input view", () => {
  const terminal: TerminalDescriptor = {
    terminalId: "terminal", terminalGeneration: 1, runtimeGeneration: "runtime", providerId: "fixture",
    workspace: "C:\\work", openedAtMs: 1, processState: "running", geometry: { columns: 80, rows: 24 },
    origin: "observedMirror", ownerInputAvailable: true,
  };
  const row = { presence: { kind: "hosted" as const, terminal }, canOpen: true, hostedTerminal: terminal };
  assert.equal(canOpenInputView(row), true);
  assert.equal(canOpenInputView({ ...row, canOpen: false }), false);
  assert.equal(canOpenInputView({ ...row, presence: { kind: "unconfirmed" } }), false);
  assert.equal(canOpenInputView({ ...row, presence: { kind: "stored", openable: true } }), false);
  assert.equal(canOpenInputView({ ...row, hostedTerminal: null }), false);
  for (const changed of [
    { origin: "owned" as const }, { processState: "stopping" as const },
    { ownerInputAvailable: false }, { ownerInputAvailable: undefined },
  ]) assert.equal(canOpenInputView({ ...row, hostedTerminal: { ...terminal, ...changed } }), false);
});
