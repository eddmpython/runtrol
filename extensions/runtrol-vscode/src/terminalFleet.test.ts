import assert from "node:assert/strict";
import { test } from "node:test";

import { TerminalFleet } from "./terminalFleet";
import type { TerminalDescriptor } from "./runtimeTypes";

function terminal(runtimeGeneration: string, terminalId: string): TerminalDescriptor {
  return {
    terminalId,
    runtimeGeneration,
    providerId: "claude",
    workspace: "C:\work\app",
    nativeSessionId: `native-${terminalId}`,
    processState: "running",
    openedAtMs: 1,
    terminalGeneration: 1,
    geometry: { columns: 120, rows: 40 },
    memoryBytes: null,
  } as TerminalDescriptor;
}

test("terminals of every generation read as one list, in one order, whichever generation answered first", () => {
  const first = new TerminalFleet();
  first.set("bbbb", { terminals: [terminal("bbbb", "t2")], warnings: [] });
  first.set("aaaa", { terminals: [terminal("aaaa", "t1")], warnings: ["aaaa partial"] });
  const second = new TerminalFleet();
  second.set("aaaa", { terminals: [terminal("aaaa", "t1")], warnings: ["aaaa partial"] });
  second.set("bbbb", { terminals: [terminal("bbbb", "t2")], warnings: [] });

  assert.deepEqual(first.merged(), second.merged());
  assert.deepEqual(first.merged().terminals.map((entry) => entry.terminalId), ["t1", "t2"]);
  assert.deepEqual(first.merged().warnings, ["aaaa partial"]);
});

test("a generation that ended takes its terminals with it and nothing else", () => {
  const fleet = new TerminalFleet();
  fleet.set("aaaa", { terminals: [terminal("aaaa", "t1")], warnings: [] });
  fleet.set("bbbb", { terminals: [terminal("bbbb", "t2")], warnings: [] });

  fleet.delete("aaaa");

  assert.deepEqual(fleet.merged().terminals.map((entry) => entry.terminalId), ["t2"]);
});

test("a generation that could not be followed is named as unknown rather than dropped in silence", () => {
  const fleet = new TerminalFleet();
  fleet.set("aaaa", { terminals: [terminal("aaaa", "t1")], warnings: [] });
  fleet.set("bbbb", { terminals: [terminal("bbbb", "t2")], warnings: [] });

  fleet.markUnreachable("bbbb", "connect ECONNREFUSED");

  const merged = fleet.merged();
  assert.deepEqual(merged.terminals.map((entry) => entry.terminalId), ["t1"]);
  assert.deepEqual(merged.warnings, ["Runtime generation bbbb could not be followed: connect ECONNREFUSED"]);

  fleet.set("bbbb", { terminals: [terminal("bbbb", "t2")], warnings: [] });
  assert.deepEqual(fleet.merged().warnings, [], "a generation followed again is no longer unknown");
});
