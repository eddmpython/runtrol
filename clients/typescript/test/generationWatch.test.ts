import assert from "node:assert/strict";
import { test } from "node:test";
import { setImmediate as turn } from "node:timers/promises";

import { watchValidatedGenerations } from "../src/generationWatch.js";

function gate(): { promise: Promise<void>; release: () => void } {
  let release!: () => void;
  return { promise: new Promise<void>((resolve) => { release = resolve; }), release };
}

type Observation = { value: string; fingerprint: string };
function engine(options: {
  hint?: (value: string, signal: AbortSignal) => Promise<string>;
  inspect?: (value: string, check: number, signal: AbortSignal) => Promise<Observation>;
  publish?: (value: string) => void | Promise<void>;
} = {}) {
  let changed!: () => void;
  let failed!: (error: unknown) => void;
  let closed = 0;
  let hints = 0;
  let checks = 0;
  let current = "a";
  const values: string[] = [];
  const abort = new AbortController();
  const done = watchValidatedGenerations(
    (change, fail) => { changed = change; failed = fail; return () => { closed += 1; }; },
    async (signal) => { hints += 1; return options.hint ? options.hint(current, signal) : current; },
    async (signal) => {
      checks += 1;
      return options.inspect ? options.inspect(current, checks, signal) : { value: current, fingerprint: current };
    },
    options.publish ?? ((value) => { values.push(value); }),
    abort.signal,
  );
  return {
    done, abort, values,
    change(value: string): void { current = value; changed(); },
    fail(error: unknown): void { failed(error); },
    get checks(): number { return checks; },
    get hints(): number { return hints; },
    get closed(): number { return closed; },
  };
}

test("generation watch publishes only validated changes and has no idle timer", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  const watch = engine();
  try {
    await turn();
    assert.deepEqual(watch.values, ["a"]);
    t.mock.timers.tick(60_000);
    await turn();
    assert.equal(watch.hints, 1);
    watch.change("a");
    await turn();
    assert.equal(watch.hints, 2);
    assert.equal(watch.checks, 1);
  } finally { watch.abort.abort(); await watch.done; }
  assert.equal(watch.closed, 1);
});

test("generation events during inspection discard obsolete results with one active inspection", async () => {
  const pending = gate();
  let active = 0;
  let maximum = 0;
  const watch = engine({ inspect: async (value, check) => {
    active += 1;
    maximum = Math.max(maximum, active);
    try { if (check === 1) await pending.promise; return { value, fingerprint: value }; }
    finally { active -= 1; }
  } });
  try {
    await turn();
    for (let n = 0; n < 20; n += 1) watch.change(String(n));
    assert.equal(watch.checks, 1);
    pending.release();
    await turn();
    assert.deepEqual(watch.values, ["19"]);
    assert.equal(watch.checks, 2);
    assert.equal(maximum, 1);
  } finally { watch.abort.abort(); pending.release(); await watch.done; }
  assert.equal(active, 0);
});

test("awaited generation consumer coalesces intermediate publications", async () => {
  const pending = gate();
  const values: string[] = [];
  const watch = engine({ publish: async (value) => {
    values.push(value);
    if (value === "a") await pending.promise;
  } });
  try {
    await turn();
    watch.change("b"); watch.change("c"); watch.change("d");
    assert.equal(watch.checks, 1);
    pending.release();
    await turn();
    assert.deepEqual(values, ["a", "d"]);
    assert.equal(watch.checks, 2);
  } finally { watch.abort.abort(); pending.release(); await watch.done; }
});

for (const stage of ["hint", "inspect", "publish"] as const) {
  test(`generation cancellation closes handles immediately and joins pending ${stage}`, async () => {
    const pending = gate();
    let entered = false;
    let joined = false;
    let observationSignal: AbortSignal | undefined;
    const pause = async <T>(name: typeof stage, value: T): Promise<T> => {
      if (stage === name) { entered = true; await pending.promise; }
      return value;
    };
    const values: string[] = [];
    const watch = engine({
      hint: (value, signal) => { observationSignal = signal; return pause("hint", value); },
      inspect: (value) => pause("inspect", { value, fingerprint: value }),
      publish: async (value) => { values.push(value); await pause("publish", undefined); },
    });
    void watch.done.then(() => { joined = true; });
    try {
      await turn();
      assert.equal(entered, true);
      watch.abort.abort();
      for (let n = 0; n < 20; n += 1) watch.change(String(n));
      assert.equal(watch.closed, 1);
      assert.equal(observationSignal?.aborted, true);
      await turn();
      assert.equal(joined, false);
      pending.release();
      await watch.done;
      assert.equal(joined, true);
      assert.deepEqual(values, stage === "publish" ? ["a"] : []);
    } finally { watch.abort.abort(); pending.release(); await watch.done; }
  });
}

test("watch source failure keeps its original error until cancelled inspection joins", async () => {
  const pending = gate();
  let signal: AbortSignal | undefined;
  const failure = new Error("owned directory fault");
  const watch = engine({ inspect: async (value, _check, inspection) => {
    signal = inspection;
    await pending.promise;
    inspection.throwIfAborted();
    return { value, fingerprint: value };
  } });
  const rejected = assert.rejects(watch.done, (error: unknown) => error === failure);
  try {
    await turn();
    watch.fail(failure);
    assert.equal(watch.closed, 1);
    assert.equal(signal?.aborted, true);
  } finally { pending.release(); await rejected; }
  assert.deepEqual(watch.values, []);
});

test("untrusted hint failure still validates and inspection failure closes observation", async () => {
  const watch = engine({ hint: async () => { throw new Error("malformed hint"); } });
  try { await turn(); assert.deepEqual(watch.values, ["a"]); }
  finally { watch.abort.abort(); await watch.done; }
  const refused = engine({ inspect: async () => { throw new Error("unsafe locator"); } });
  await assert.rejects(refused.done, /unsafe locator/);
  assert.equal(refused.closed, 1);
});

test("pre-cancel arms nothing and synchronous source failure releases its handle", async () => {
  const abort = new AbortController();
  abort.abort();
  await watchValidatedGenerations(
    () => { throw new Error("unexpected arm"); },
    async () => { throw new Error("unexpected hint"); },
    async () => { throw new Error("unexpected inspection"); },
    () => assert.fail("unexpected publication"), abort.signal,
  );
  let closed = 0;
  await assert.rejects(watchValidatedGenerations(
    (_changed, failed) => { failed(new Error("arm failed")); return () => { closed += 1; }; },
    async () => "a", async () => ({ value: "a", fingerprint: "a" }), () => {},
  ), /arm failed/);
  assert.equal(closed, 1);
});

test("an event during arming is retained and consumer refusal closes observation", async () => {
  let closed = 0;
  let inspections = 0;
  await assert.rejects(watchValidatedGenerations(
    (changed) => { changed(); return () => { closed += 1; }; }, async () => "a",
    async () => { inspections += 1; return { value: "a", fingerprint: "a" }; },
    async () => { throw new Error("consumer refused"); },
  ), /consumer refused/);
  assert.equal(inspections, 1);
  assert.equal(closed, 1);
});
