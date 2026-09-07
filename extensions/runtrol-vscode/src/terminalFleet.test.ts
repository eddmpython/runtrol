import assert from "node:assert/strict";
import { test } from "node:test";

import { TerminalFleet } from "./terminalFleet";
import type { TerminalDescriptor } from "./runtimeTypes";
import type { RuntimeGenerationSnapshot, ValidatedLocator } from "@runtrol/runtime-client";
import type { TerminalIndexSnapshot } from "./runtimeTypes";

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

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function generation(digest: string, revision = "first", draining = false): ValidatedLocator {
  return { digest, revision, draining } as ValidatedLocator;
}
function snapshot(...generations: ValidatedLocator[]): RuntimeGenerationSnapshot {
  return { current: { state: "notInstalled" }, currentRevision: null, generations } as RuntimeGenerationSnapshot;
}
function harness(automaticCleanup = true) {
  const fleet = new TerminalFleet();
  const abort = new AbortController();
  const source = deferred();
  let receive!: (snapshot: RuntimeGenerationSnapshot) => void;
  let publications = 0;
  let active = 0;
  let maximum = 0;
  const runs: {
    generation: ValidatedLocator;
    signal: AbortSignal;
    receive: (snapshot: TerminalIndexSnapshot) => void;
    done: ReturnType<typeof deferred>;
  }[] = [];
  const watching = fleet.followGenerations((callback, signal) => {
    receive = callback;
    signal.addEventListener("abort", source.resolve, { once: true });
    return source.promise;
  }, async (generation, callback, signal) => {
    const done = deferred();
    runs.push({ generation, signal, receive: callback, done });
    active += 1;
    maximum = Math.max(maximum, active);
    if (automaticCleanup) signal.addEventListener("abort", done.resolve, { once: true });
    try { await done.promise; } finally { active -= 1; }
  }, () => { publications += 1; }, abort.signal);
  return {
    fleet, abort, source, runs, watching,
    send: (value: RuntimeGenerationSnapshot) => receive(value),
    get publications() { return publications; },
    get active() { return active; },
    get maximum() { return maximum; },
    async stop() {
      abort.abort();
      for (const run of runs) run.done.resolve();
      await watching;
    },
  };
}

test("all generations start from one source event without an idle locator timer", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  const timers = context.mock.method(globalThis, "setTimeout");
  const h = harness();
  try {
    await flush();
    h.send(snapshot(generation("primary"), generation("draining", "first", true)));
    await flush();
    assert.equal(h.runs.length, 2);
    h.runs[0].receive({ terminals: [terminal("primary", "one")], warnings: [] });
    h.runs[1].receive({ terminals: [terminal("draining", "two")], warnings: [] });
    context.mock.timers.tick(180_000);
    await flush();
    assert.equal(h.runs.length, 2, "healthy watches never relist or reconnect on a clock");
    assert.equal(timers.mock.calls.length, 0, "idle observation does not allocate a polling timer");
    assert.deepEqual(h.fleet.merged().terminals.map((row) => row.terminalId), ["two", "one"]);
  } finally { await h.stop(); }
});

test("same revision draining and terminal-count updates retain the existing stream", async () => {
  const h = harness();
  try {
    await flush();
    h.send(snapshot(generation("same")));
    await flush();
    h.send(snapshot(generation("same", "first", true)));
    await flush();
    assert.equal(h.runs.length, 1);
    assert.equal(h.runs[0].signal.aborted, false);
  } finally { await h.stop(); }
});

test("same digest restart waits for old cancellation and rejects stale publication and failure", async () => {
  const h = harness(false);
  try {
    await flush();
    h.send(snapshot(generation("same")));
    await flush();
    const old = h.runs[0];
    old.receive({ terminals: [terminal("same", "old")], warnings: [] });
    h.send(snapshot(generation("same", "second")));
    await flush();
    assert.equal(old.signal.aborted, true);
    assert.equal(h.runs.length, 1, "old in-flight work retains its only admission slot");
    assert.deepEqual(h.fleet.merged().terminals, []);
    old.receive({ terminals: [terminal("same", "late")], warnings: [] });
    assert.deepEqual(h.fleet.merged().terminals, []);
    old.done.reject(new Error("stale old failure"));
    await flush();
    assert.equal(h.runs.length, 2);
    h.runs[1].receive({ terminals: [terminal("same", "new")], warnings: [] });
    old.receive({ terminals: [], warnings: ["stale warning"] });
    await flush();
    assert.deepEqual(h.fleet.merged().terminals.map((row) => row.terminalId), ["new"]);
    assert.deepEqual(h.fleet.merged().warnings, []);
    assert.equal(h.maximum, 1);
  } finally { await h.stop(); }
});

test("replacement bursts coalesce to one latest snapshot while shrinking and growing", async () => {
  const h = harness(false);
  try {
    await flush();
    h.send(snapshot(generation("a"), generation("b"), generation("c")));
    await flush();
    h.send(snapshot(generation("a", "second")));
    await flush();
    assert.equal(h.runs.length, 3);
    for (let revision = 3; revision < 200; revision += 1) {
      h.send(snapshot(generation("a", String(revision))));
    }
    h.runs[0].done.resolve();
    h.runs[1].done.resolve();
    await flush();
    assert.equal(h.runs.length, 3, "one retiring worker still fills the shrunken width");
    h.runs[2].done.resolve();
    await flush();
    assert.equal(h.runs.length, 4);
    assert.equal(h.runs[3].generation.revision, "199");
    h.send(snapshot(generation("a", "199"), generation("d")));
    await flush();
    assert.equal(h.runs.length, 5, "growth admits a new generation immediately");
    assert.equal(h.active, 2);
    assert.equal(h.maximum, 3, "no second cohort or pending run queue was allocated");
  } finally { await h.stop(); }
});

test("a failed generation retries after the existing cooldown while a new generation starts immediately", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  const h = harness();
  try {
    await flush();
    h.send(snapshot(generation("a")));
    await flush();
    h.runs[0].done.reject(new Error("connect ECONNREFUSED"));
    await flush();
    assert.deepEqual(h.fleet.merged().warnings,
      ["Runtime generation a could not be followed: connect ECONNREFUSED"]);
    h.send(snapshot(generation("a", "first", true), generation("b")));
    await flush();
    assert.deepEqual(h.runs.map((run) => run.generation.digest), ["a", "b"]);
    context.mock.timers.tick(14_999);
    await flush();
    assert.equal(h.runs.length, 2);
    context.mock.timers.tick(1);
    await flush();
    assert.equal(h.runs.length, 3);
    h.runs[2].receive({ terminals: [terminal("a", "recovered")], warnings: [] });
    assert.deepEqual(h.fleet.merged().warnings, []);
  } finally { await h.stop(); }
});

test("an unexpectedly completed stream also backs off instead of spinning", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  const h = harness();
  try {
    await flush();
    h.send(snapshot(generation("a")));
    await flush();
    h.runs[0].done.resolve();
    await flush();
    assert.equal(h.runs.length, 1);
    assert.match(h.fleet.merged().warnings[0], /stopped unexpectedly/);
    context.mock.timers.tick(15_000);
    await flush();
    assert.equal(h.runs.length, 2);
  } finally { await h.stop(); }
});

test("crossing the retry deadline during reconciliation still schedules recovery", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  const h = harness();
  try {
    await flush();
    h.send(snapshot(generation("a")));
    await flush();
    h.runs[0].done.reject(new Error("connection failed"));
    await flush();
    let readings = 0;
    context.mock.method(Date, "now", () => ++readings === 1 ? 14_999 : 15_000);
    h.send(snapshot(generation("a")));
    await flush();
    context.mock.timers.tick(0);
    await flush();
    assert.equal(h.runs.length, 2, "the deadline cannot remove both admission and its retry wakeup");
  } finally { await h.stop(); }
});

test("cancel during pending connection joins cleanup and ignores a late source or worker result", async () => {
  const h = harness(false);
  await flush();
  h.send(snapshot(generation("a")));
  await flush();
  let finished = false;
  void h.watching.then(() => { finished = true; });
  h.abort.abort();
  await flush();
  assert.equal(h.runs[0].signal.aborted, true);
  assert.equal(finished, false, "the pending connection must finish cleanup before returning");
  h.send(snapshot(generation("b")));
  h.runs[0].receive({ terminals: [terminal("a", "late")], warnings: [] });
  assert.equal(h.publications, 0);
  h.runs[0].done.resolve();
  await h.watching;
  assert.equal(h.runs.length, 1);
  assert.equal(h.active, 0);
});

test("source failure propagates after aborting and joining each generation", async () => {
  const h = harness(false);
  const failure = new Error("locator verification failed");
  const rejected = assert.rejects(h.watching, (error: unknown) => error === failure);
  await flush();
  h.send(snapshot(generation("a")));
  await flush();
  h.source.reject(failure);
  await flush();
  assert.equal(h.runs[0].signal.aborted, true);
  h.runs[0].done.resolve();
  await rejected;
  assert.equal(h.active, 0);
});

test("already cancelled follow starts neither source nor workers", async () => {
  const abort = new AbortController();
  abort.abort();
  await new TerminalFleet().followGenerations(
    async () => { assert.fail("source started"); },
    async () => { assert.fail("worker started"); },
    () => { assert.fail("publication started"); }, abort.signal,
  );
});

test("unexpected source completion fails instead of silently leaving a frozen fleet", async () => {
  const h = harness();
  const rejected = assert.rejects(h.watching, /Runtime generation observation stopped unexpectedly/);
  await flush();
  h.source.resolve();
  await rejected;
});

test("a failing publication ends the owner watch and joins its connections", async () => {
  const failure = new Error("publication failed");
  let closed = false;
  await assert.rejects(new TerminalFleet().followGenerations(async (receive, signal) => {
    receive(snapshot(generation("a")));
    await new Promise<void>((resolve) => signal.addEventListener("abort", () => {
      closed = true;
      resolve();
    }, { once: true }));
  }, async () => { throw new Error("connection failed"); }, () => { throw failure; },
  new AbortController().signal), (error: unknown) => error === failure);
  assert.equal(closed, true);
});

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
