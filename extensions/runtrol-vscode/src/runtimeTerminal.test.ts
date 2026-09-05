import assert from "node:assert/strict";
import test from "node:test";

import {
  PUBLIC_LIMITS,
  RuntimeProtocolError,
  RuntimeRequestError,
  RuntimeTransportError,
  newMutationRequestId,
  type TerminalControlLease,
  type TerminalDescriptor,
  type TerminalView,
} from "@runtrol/runtime-client";

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

/// A view that plays a scripted sequence of notifications, as the Runtime client would deliver them, and records
/// every byte string written through it.
function fakeView(initialScreen: string, steps: Step[], writes: Uint8Array[] = []): TerminalView {
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
    async write(params: { bytesBase64: string }) { writes.push(new Uint8Array(Buffer.from(params.bytesBase64, "base64"))); },
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
  const openings: Promise<void>[] = [];
  const closed: Array<number | void> = [];
  const attachments: Array<{ generation: string; terminal: string }> = [];
  const disconnected: string[] = [];
  let calls = 0;
  const runtime = {
    openTerminal: async () => {
      const open = opens[calls] ?? opens[opens.length - 1]!;
      calls += 1;
      return open();
    },
    attachTerminal: async (generation: string, terminal: string) => {
      attachments.push({ generation, terminal });
      const open = opens[calls] ?? opens[opens.length - 1]!;
      calls += 1;
      return open();
    },
  } as unknown as StudioRuntimeClient;
  const presentation: TerminalPresentation = {
    opening: (work) => { shown.push("opening"); openings.push(work); },
    ended: (code) => shown.push(`ended ${code}`),
    failed: (message) => shown.push(`failed ${message}`),
  };
  const pty = new RuntimeTerminal(runtime, target, () => undefined, async () => null, () => undefined, (reason) => disconnected.push(reason), presentation);
  pty.onDidWrite((text) => written.push(text));
  pty.onDidClose((code) => closed.push(code));
  return { pty, written, shown, closed, openings, attachments, disconnected };
}

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const continuations = () => new Promise<void>((resolve) => setImmediate(resolve));

for (const failureKind of ["transport", "outcomeUnknown"] as const) {
  for (const failureAt of ["before output breaks", "while output resolves", "while attaching", "after replacement"] as const) {
    test(`a ${failureKind} write failure ${failureAt} cannot end exact-view recovery or replay input`, async () => {
      const output = deferred<Awaited<ReturnType<TerminalView["next"]>>>();
      const write = deferred();
      const writing = deferred();
      const attaching = deferred();
      const replacement = deferred<TerminalView>();
      const oldWrites: string[] = [];
      const newWrites: Uint8Array[] = [];
      let replacementClosed = false;
      const failsFirst = failureAt === "before output breaks" || failureAt === "while output resolves";
      const old = Object.assign(fakeView("old", []), {
        next: () => output.promise,
        close: () => {
          if (failureAt === "while output resolves") {
            output.resolve({ kind: "output", sequence: 1, bytes: encoder.encode("obsolete") });
          } else {
            output.reject(new RuntimeTransportError("old view closed"));
          }
        },
        write: (params: { bytesBase64: string }) => {
          oldWrites.push(Buffer.from(params.bytesBase64, "base64").toString("utf8"));
          writing.resolve();
          return write.promise;
        },
      });
      const next = Object.assign(fakeView("replacement", [{ kind: "hang" }], newWrites), {
        close: () => { replacementClosed = true; },
      });
      const { pty, openings, written, shown, attachments, disconnected } = harness([
        async () => old,
        () => { attaching.resolve(); return replacement.promise; },
      ]);
      try {
        pty.open(undefined);
        await openings[0];
        const error = failureKind === "transport"
          ? new RuntimeTransportError("write connection ended")
          : new RuntimeRequestError({ code: "outcomeUnknown", correlationId: newMutationRequestId(),
              message: "The mutation outcome is unknown; a controlConflict response was not received", retryable: false });
        const failed = assert.rejects(pty.handleMeasuredInput("uncertain"), (reason) => reason === error);
        await writing.promise;
        if (failsFirst) {
          write.reject(error);
          await failed;
          await continuations();
          assert.deepEqual(disconnected, [], "a control failure leaves recovery to the output pump");
        } else {
          output.reject(new RuntimeTransportError("output connection ended"));
          await attaching.promise;
        }
        const queued = failureAt === "while attaching"
          ? pty.handleMeasuredInput("queued").then(() => "acknowledged", (reason: unknown) => reason)
          : null;
        if (failureAt === "after replacement") {
          replacement.resolve(next);
          await continuations();
        }
        if (!failsFirst) {
          write.reject(error);
          await failed;
        }
        if (failureAt !== "after replacement") replacement.resolve(next);
        await continuations();
        assert.equal(replacementClosed, false, "the old mutation cannot close the replacement view");
        assert.ok(pty.descriptor(), "the exact terminal remains attached");
        if (queued) assert.equal(await queued, "acknowledged", "unsent input waits for the replacement");
        await pty.handleMeasuredInput("later");
        assert.deepEqual(oldWrites, ["uncertain"], "an unresolved write is never retried");
        assert.deepEqual(newWrites.map((bytes) => Buffer.from(bytes).toString("utf8")), queued ? ["queued", "later"] : ["later"]);
        assert.deepEqual(attachments, [{ generation: "gen-1", terminal: "term-1" }]);
        assert.deepEqual(written, ["old", "replacement"]);
        assert.deepEqual(shown, ["opening"]);
        assert.deepEqual(disconnected, []);
      } finally {
        pty.close();
      }
    });
  }
}

for (const [leaseAction, leaseOutcome] of [
  ["acquire", "granted"], ["renew", "granted"], ["acquire", "refused"], ["renew", "refused"],
] as const) {
  test(`a delayed ${leaseAction} ${leaseOutcome} answer cannot install an old lease or dispatch through its retired view`, async () => {
    const output = deferred<Awaited<ReturnType<TerminalView["next"]>>>();
    const lease = deferred<TerminalControlLease>();
    const asking = deferred();
    let oldLeaseRequests = 0;
    let oldWrites = 0;
    let acquired = 0;
    const newWrites: string[] = [];
    const old = fakeView("old", []);
    Object.assign(old, {
      opened: { ...old.opened, controlLease: leaseAction === "acquire" ? null : {
        ...old.opened.controlLease, expiresAtMs: Date.now() + 1_000,
      } },
      next: () => output.promise,
      close: () => output.reject(new RuntimeTransportError("old view closed")),
      acquireControl: () => { oldLeaseRequests += 1; asking.resolve(); return lease.promise; },
      renewControl: () => { oldLeaseRequests += 1; asking.resolve(); return lease.promise; },
      write: async () => { oldWrites += 1; },
    });
    const freshLease = { leaseId: "fresh", leaseGeneration: 2, expiresAtMs: Date.now() + 60_000 } as unknown as TerminalControlLease;
    const next = Object.assign(fakeView("replacement", [{ kind: "hang" }]), {
      acquireControl: async () => { acquired += 1; return freshLease; },
      write: async (params: { leaseId: string; bytesBase64: string }) => {
        assert.equal(params.leaseId, freshLease.leaseId);
        newWrites.push(Buffer.from(params.bytesBase64, "base64").toString("utf8"));
      },
    });
    const { pty, openings, disconnected } = harness([async () => old, async () => next]);
    try {
      pty.open(undefined);
      await openings[0];
      const failed = assert.rejects(pty.handleMeasuredInput("old"), leaseOutcome === "refused"
        ? /controlConflict/u : /view changed before control dispatch/u);
      await asking.promise;
      output.reject(new RuntimeTransportError("output connection ended"));
      await continuations();
      if (leaseOutcome === "refused") {
        lease.reject(new RuntimeRequestError({ code: "controlConflict", correlationId: newMutationRequestId(),
          message: "the retired view no longer owns the lease", retryable: false }));
      } else {
        lease.resolve({ ...freshLease, leaseId: "obsolete" } as unknown as TerminalControlLease);
      }
      await failed;
      await pty.handleMeasuredInput("later");
      assert.equal(oldWrites, 0, "an obsolete lease never authorizes a write");
      assert.equal(oldLeaseRequests, 1, "a retired view cannot ask again after a delayed lease refusal");
      assert.equal(acquired, 1, "the replacement acquires its own lease");
      assert.deepEqual(newWrites, ["later"]);
      assert.deepEqual(disconnected, []);
    } finally {
      pty.close();
    }
  });
}

test("closing during reconnect rejects unsent input and closes a late replacement without drawing it", async () => {
  const output = deferred<Awaited<ReturnType<TerminalView["next"]>>>();
  const attaching = deferred();
  const replacement = deferred<TerminalView>();
  const writes: Uint8Array[] = [];
  let replacementClosed = false;
  const old = Object.assign(fakeView("old", []), {
    next: () => output.promise,
    close: () => output.reject(new RuntimeTransportError("old closed")),
  });
  const next = Object.assign(fakeView("replacement", [{ kind: "hang" }], writes), {
    close: () => { replacementClosed = true; },
  });
  const { pty, openings, written } = harness([async () => old, () => { attaching.resolve(); return replacement.promise; }]);
  pty.open(undefined);
  await openings[0];
  output.reject(new RuntimeTransportError("output connection ended"));
  await attaching.promise;
  const input = assert.rejects(pty.handleMeasuredInput("unsent"), /closed before measured input dispatch/u);
  const armed = assert.rejects(pty.measureNextInput(), /closed before measured input arrived/u);
  pty.close();
  await Promise.all([input, armed]);
  replacement.resolve(next);
  await continuations();
  assert.equal(replacementClosed, true);
  assert.deepEqual(written, ["old"]);
  assert.deepEqual(writes, []);
});

test("reconnecting input keeps byte and action bounds and a failed reattach ends waiting measurements", async () => {
  for (const outcome of ["failedAttach", "byteOverflow", "actionOverflow"] as const) {
    const output = deferred<Awaited<ReturnType<TerminalView["next"]>>>();
    const attaching = deferred();
    const replacement = deferred<TerminalView>();
    const old = Object.assign(fakeView("old", []), {
      next: () => output.promise,
      close: () => output.reject(new RuntimeTransportError("old closed")),
    });
    const { pty, openings, shown, disconnected } = harness([async () => old, () => { attaching.resolve(); return replacement.promise; }]);
    try {
      pty.open(undefined);
      await openings[0];
      output.reject(new RuntimeTransportError("output connection ended"));
      await attaching.promise;
      if (outcome !== "failedAttach") {
        if (outcome === "actionOverflow") {
          for (let count = 0; count < 256; count += 1) pty.handleInput("x");
        }
        const input = outcome === "byteOverflow" ? "x".repeat(PUBLIC_LIMITS.maxTerminalWriteBytes + 1) : "x";
        await assert.rejects(pty.handleMeasuredInput(input), /bounded control queue/u);
        assert.ok(shown.some((message) => message.includes("bounded control queue")));
      } else {
        const input = assert.rejects(pty.handleMeasuredInput("unsent"), /closed before measured input dispatch/u);
        replacement.reject(new RuntimeTransportError("exact generation is unavailable"));
        await input;
        assert.deepEqual(shown, ["opening"], "reachability remains the index watch's message");
      }
      assert.equal(disconnected.length, 1);
      assert.equal(pty.descriptor(), null);
    } finally {
      pty.close();
      replacement.reject(new RuntimeTransportError("test attachment ended"));
    }
  }
});

for (const ErrorClass of [Error, RuntimeProtocolError]) {
  test(`${ErrorClass.name} text cannot authorize replay of an unacknowledged input`, async () => {
    const error = new ErrorClass("controlConflict appeared in an invalid response before the outcome was known");
    const writes: string[] = [];
    let acquired = 0;
    const view = Object.assign(fakeView("screen", [{ kind: "hang" }]), {
      write: async (params: { bytesBase64: string }) => {
        writes.push(Buffer.from(params.bytesBase64, "base64").toString("utf8"));
        throw error;
      },
      acquireControl: async () => { acquired += 1; return view.opened.controlLease!; },
    });
    const { pty, openings, shown, attachments, disconnected } = harness([async () => view]);
    try {
      pty.open(undefined);
      await openings[0];
      await assert.rejects(pty.handleMeasuredInput("uncertain"), (reason) => reason === error);
      assert.deepEqual(writes, ["uncertain"], "an error message cannot prove the write was refused");
      assert.equal(acquired, 0, "unknown failures cannot request a replacement lease");
      assert.deepEqual(attachments, []);
      assert.deepEqual(shown, ["opening", `failed ${error.message}`]);
      assert.equal(disconnected.length, 1);
    } finally {
      pty.close();
    }
  });
}

for (const code of ["leaseExpired", "controlConflict"] as const) {
  test(`a typed ${code} refusal permits one fresh lease and one new write attempt`, async () => {
    const writes: string[] = [];
    let acquired = 0;
    const view = Object.assign(fakeView("screen", [{ kind: "hang" }]), {
      write: async (params: { bytesBase64: string }) => {
        writes.push(Buffer.from(params.bytesBase64, "base64").toString("utf8"));
        if (writes.length === 1) {
          throw new RuntimeRequestError({ code, correlationId: newMutationRequestId(),
            message: "the write was refused because the lease is no longer current", retryable: false });
        }
      },
      acquireControl: async () => { acquired += 1; return view.opened.controlLease!; },
    });
    const { pty, openings, shown, disconnected } = harness([async () => view]);
    try {
      pty.open(undefined);
      await openings[0];
      await pty.handleMeasuredInput("refused once");
      assert.deepEqual(writes, ["refused once", "refused once"]);
      assert.equal(acquired, 1);
      assert.deepEqual(shown, ["opening"]);
      assert.deepEqual(disconnected, []);
    } finally {
      pty.close();
    }
  });
}

test("a genuine mutation refusal on the current view is still reported and closes that route", async () => {
  const view = Object.assign(fakeView("screen", [{ kind: "hang" }]), {
    write: async () => { throw new Error("permissionDenied: terminal input was refused"); },
  });
  const { pty, openings, shown, attachments, disconnected } = harness([async () => view]);
  try {
    pty.open(undefined);
    await openings[0];
    await assert.rejects(pty.handleMeasuredInput("refused"), /permissionDenied/u);
    assert.deepEqual(attachments, []);
    assert.deepEqual(shown, ["opening", "failed permissionDenied: terminal input was refused"]);
    assert.equal(disconnected.length, 1);
  } finally {
    pty.close();
  }
});

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

test("opening settles while a connected provider remains alive and keeps delivering output", async () => {
  const { pty, openings, written, closed } = harness([
    async () => fakeView("ready", [{ kind: "output", text: "still running" }, { kind: "hang" }]),
  ]);
  pty.open(undefined);
  const outcome = await Promise.race([
    openings[0]!.then(() => "connected"),
    settle(100).then(() => "still opening"),
  ]);
  assert.equal(outcome, "connected");
  assert.deepEqual(written, ["ready", "still running"]);
  assert.deepEqual(closed, []);
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

test("a follower's pane resize never takes control; typing does, and the pane's size follows the transfer", async () => {
  const calls: string[] = [];
  let resizeRefusals = 1;
  const view = fakeView("", [{ kind: "hang" }]);
  const traced = Object.assign(view, {
    async write() { calls.push("write"); },
    async acquireControl() {
      calls.push("acquire");
      return { leaseId: "lease-2", leaseGeneration: 2, expiresAtMs: Date.now() + 60_000 } as unknown as TerminalControlLease;
    },
    async resize() {
      calls.push("resize");
      // The Runtime refuses a resize from a view that no longer holds control.
      if (resizeRefusals > 0) {
        resizeRefusals -= 1;
        throw new RuntimeRequestError({ code: "controlConflict", correlationId: newMutationRequestId(),
          message: "another view holds control of this terminal", retryable: false });
      }
    },
  });
  const { pty } = harness([async () => traced]);
  // The open request itself carries the pane's size, so a view that opens holding control sends no resize.
  pty.open({ columns: 100, rows: 30 });
  await settle();
  assert.deepEqual(calls, []);
  // Another window took control meanwhile: this pane's resize is refused once and it does not take control back.
  pty.setDimensions({ columns: 90, rows: 25 });
  await settle();
  assert.deepEqual(calls, ["resize"], "a holder-turned-follower is refused once and asks for nothing");
  calls.length = 0;
  // Now a follower: a further pane change sends nothing at all.
  pty.setDimensions({ columns: 80, rows: 20 });
  await settle();
  assert.deepEqual(calls, [], "a follower never resizes the shared process");
  // Typing takes control, writes once, and only then sends this pane's size.
  pty.handleInput("k");
  await settle(80);
  assert.deepEqual(calls, ["acquire", "write", "resize"]);
  pty.close();
});

test("every key, paste, IME result, Escape, interrupt, and mouse report is written once, in order, unchanged", async () => {
  const writes: Uint8Array[] = [];
  const { pty } = harness([async () => fakeView("", [{ kind: "hang" }], writes)]);
  const typed = ["안녕하세요 ", "hello", "\r", "pasted line one\nline two\r", "\x1b", "\x03", "\x1b[<0;10;5M"];
  // Two keys land before the view is connected; they are queued and sent first, once.
  pty.handleInput(typed[0]!);
  pty.handleInput(typed[1]!);
  pty.open(undefined);
  await settle();
  for (const text of typed.slice(2)) pty.handleInput(text);
  await settle(80);
  const decoder = new TextDecoder("utf-8");
  assert.deepEqual(writes.map((bytes) => decoder.decode(bytes)), typed);
  pty.close();
});
