import assert from "node:assert/strict";
import test from "node:test";

import type { TerminalControlLease, TerminalDescriptor, TerminalView } from "@runtrol/runtime-client";

import { RuntimeTerminal, type Target, type TerminalPresentation } from "./runtimeTerminal";
import type { StudioRuntimeClient } from "./runtimeClient";

const encoder = new TextEncoder();

const descriptor: TerminalDescriptor = {
  terminalId: "term-1",
  runtimeGeneration: "gen-1",
  terminalGeneration: 1,
  providerId: "claude",
  workspace: "C:\\work",
  processState: "Running",
  processId: 1,
  geometry: { columns: 120, rows: 40 },
  attachedViews: 1,
  nativeSessionId: null,
} as unknown as TerminalDescriptor;

type Step =
  | { kind: "output"; text: string }
  | { kind: "lagged"; screen: string }
  | { kind: "exited"; exitCode: number }
  | { kind: "break" }
  | { kind: "hang" };

/// A view that plays a scripted sequence of notifications, as the Runtime client would deliver them.
function fakeView(initialScreen: string, steps: Step[]): TerminalView {
  const queue = [...steps];
  const lease: TerminalControlLease = {
    leaseId: "lease-1",
    leaseGeneration: 1,
    expiresAtMs: Date.now() + 60_000,
  } as unknown as TerminalControlLease;
  return {
    opened: { terminal: descriptor, viewId: "view-1", controlLease: lease, screenBase64: "" },
    initialScreen: encoder.encode(initialScreen),
    async next() {
      const step = queue.shift();
      if (!step || step.kind === "hang") return new Promise(() => undefined);
      if (step.kind === "break") throw new Error("connection ended");
      if (step.kind === "output") return { kind: "output", sequence: 1, bytes: encoder.encode(step.text) };
      if (step.kind === "lagged") return { kind: "lagged", lostChunks: 1, screen: encoder.encode(step.screen), nextSequence: 2 };
      return { kind: "exited", exitCode: step.exitCode };
    },
    close() {},
    async write() {},
    async acquireControl() { return lease; },
    async renewControl() { return lease; },
    async resize() {},
    async detach() {},
  } as unknown as TerminalView;
}

const target: Target = { provider: "claude", native: null, hosted: null, workspace: "C:\\work", blocked: null };

function harness(opens: Array<() => Promise<TerminalView>>) {
  const written: string[] = [];
  const shown: string[] = [];
  const closed: Array<number | void> = [];
  let calls = 0;
  const runtime = {
    openTerminal: async () => {
      const open = opens[calls] ?? opens[opens.length - 1]!;
      calls += 1;
      return open();
    },
    attachTerminal: async () => {
      const open = opens[calls] ?? opens[opens.length - 1]!;
      calls += 1;
      return open();
    },
  } as unknown as StudioRuntimeClient;
  const presentation: TerminalPresentation = {
    opening: () => shown.push("opening"),
    ended: (code) => shown.push(`ended ${code}`),
    failed: (message) => shown.push(`failed ${message}`),
  };
  const pty = new RuntimeTerminal(runtime, target, () => undefined, async () => null, () => undefined, () => undefined, presentation);
  pty.onDidWrite((text) => written.push(text));
  pty.onDidClose((code) => closed.push(code));
  return { pty, written, shown, closed };
}

const settle = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

test("a cold open writes nothing until the Runtime's own screen arrives, then only the service's bytes", async () => {
  const { pty, written, shown } = harness([async () => fakeView("\x1b[H\x1b[Jwelcome", [{ kind: "output", text: "> " }, { kind: "hang" }])]);
  pty.open({ columns: 100, rows: 30 });
  assert.deepEqual(written, [], "nothing is written while the terminal opens");
  await settle();
  assert.deepEqual(written, ["\x1b[H\x1b[Jwelcome", "> "]);
  assert.deepEqual(shown, ["opening"]);
  pty.close();
});

test("a failed open writes nothing into the pane and is told to the workbench", async () => {
  const { pty, written, shown } = harness([async () => { throw new Error("no provider called nothing"); }]);
  pty.open(undefined);
  await settle();
  assert.deepEqual(written, []);
  assert.deepEqual(shown, ["opening", "failed no provider called nothing"]);
});

test("a lag replacement and a reconnect write the Runtime's checkpoint bytes only, with no clear of Studio's own", async () => {
  const { pty, written } = harness([
    async () => fakeView("\x1b[H\x1b[Jfirst", [{ kind: "lagged", screen: "\x1b[H\x1b[Jreplaced" }, { kind: "break" }]),
    async () => fakeView("\x1b[H\x1b[Jreattached", [{ kind: "hang" }]),
  ]);
  pty.open(undefined);
  await settle();
  assert.deepEqual(written, ["\x1b[H\x1b[Jfirst", "\x1b[H\x1b[Jreplaced", "\x1b[H\x1b[Jreattached"]);
  pty.close();
});

test("a provider exit closes a clean tab and leaves a failed one standing with no sentence written", async () => {
  const clean = harness([async () => fakeView("", [{ kind: "exited", exitCode: 0 }])]);
  clean.pty.open(undefined);
  await settle();
  assert.deepEqual(clean.written, [""]);
  assert.deepEqual(clean.closed, [0]);
  const dirty = harness([async () => fakeView("last words", [{ kind: "exited", exitCode: 3 }])]);
  dirty.pty.open(undefined);
  await settle();
  assert.deepEqual(dirty.written, ["last words"]);
  assert.deepEqual(dirty.shown, ["opening", "ended 3"]);
  assert.deepEqual(dirty.closed, []);
});

test("the pane's one exception still holds: mouse-mode switches never reach VS Code's terminal", async () => {
  const { pty, written } = harness([async () => fakeView("\x1b[?1000h\x1b[?1006hscreen", [{ kind: "output", text: "\x1b[?1049;1000hmore" }, { kind: "hang" }])]);
  pty.open(undefined);
  await settle();
  assert.deepEqual(written, ["screen", "\x1b[?1049hmore"]);
  pty.close();
});
