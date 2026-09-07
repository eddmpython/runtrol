import assert from 'node:assert/strict';
import {test} from 'node:test';
import {RuntimeGenerations} from './runtimeGenerations';
import type {RuntimeGenerationSnapshot} from '@runtrol/runtime-client';
import {validatedLocator} from '../../../clients/typescript/src/testing';
type Watch = {publish: (snapshot: RuntimeGenerationSnapshot) => void | Promise<void>; reject: (error: unknown) => void; resolve: () => void; stopped: boolean};
async function until(check: () => boolean) {
  for (let n = 0; n < 100; n++) {
    if (check()) return;
    await new Promise(resolve => setImmediate(resolve));
  }
  throw new Error('bounded observation deadline');
}

function fixture() {
  const watches: Watch[] = [], observed: RuntimeGenerationSnapshot[] = [], failed: unknown[] = [];
  const generations = new RuntimeGenerations(() => ({
    watchGenerations(publish, {signal} = {}) {
      assert.ok(signal);
      return new Promise<void>((resolve, reject) => {
        const watch = {publish, reject, resolve, stopped: false};
        signal.addEventListener('abort', () => { watch.stopped = true; resolve(); }, {once: true});
        watches.push(watch);
      });
    },
  }), value => observed.push(value), error => failed.push(error));
  return {generations, watches, observed, failed};
}
function snapshot(value: string): RuntimeGenerationSnapshot {
  const locator = validatedLocator('home', 'pipe-' + value, '0.1.1', value.padEnd(64, '0'));
  return {current: {state: 'running', locator}, currentRevision: value, generations: [locator]};
}

test('one Studio watch supplies concurrent first inspections and all index consumers', async () => {
  const f = fixture(), abort = new AbortController();
  const values: (string | null)[] = [];
  const following = f.generations.follow(value => values.push(value.currentRevision), abort.signal);
  const initial = Array.from({length: 50}, () => f.generations.snapshot());
  await until(() => f.watches.length === 1);
  assert.equal(f.watches.length, 1);
  f.watches[0].publish(snapshot('a'));
  assert.equal((await Promise.all(initial)).every(value => value.currentRevision === 'a'), true);
  f.watches[0].publish(snapshot('b'));
  assert.deepEqual(values, ['a', 'b']);
  assert.equal((await f.generations.snapshot()).currentRevision, 'b');
  abort.abort();
  await following;
  assert.equal(f.watches[0].stopped, false, 'source lifetime is Studio, not one restarting index consumer');
  f.generations.close();
  assert.equal(f.watches[0].stopped, true);
});

test('failed native validation invalidates every cached snapshot and next consumer starts one recovery watch', async () => {
  const f = fixture(), abort = new AbortController();
  const ended = assert.rejects(f.generations.follow(() => {}, abort.signal), /native owner/);
  await until(() => f.watches.length === 1);
  f.watches[0].publish(snapshot('a'));
  f.watches[0].reject(new Error('native owner changed'));
  await ended;
  assert.equal(f.generations.latest, null);
  assert.equal(f.failed.length, 1);
  const first = f.generations.snapshot(), second = f.generations.snapshot();
  await until(() => f.watches.length === 2);
  assert.equal(f.watches.length, 2);
  f.watches[1].publish(snapshot('b'));
  assert.equal((await first).currentRevision, 'b');
  assert.equal((await second).currentRevision, 'b');
  f.generations.close();
});

test('changing verifier source suppresses old late validation and starts fresh observation', async () => {
  const f = fixture();
  const old = assert.rejects(f.generations.snapshot(), /verification source changed/);
  await until(() => f.watches.length === 1);
  f.generations.reset();
  await old;
  const next = f.generations.snapshot();
  await until(() => f.watches.length === 2);
  assert.equal(f.watches[0].stopped, true);
  f.watches[0].publish(snapshot('obsolete'));
  assert.equal(f.generations.latest, null);
  assert.deepEqual(f.observed, []);
  f.watches[1].publish(snapshot('new'));
  assert.equal((await next).currentRevision, 'new');
  f.generations.close();
});

test('cancelling a waiting command does not cancel another consumer or reopen the OS watcher', async () => {
  const f = fixture(), cancelled = new AbortController();
  const gone = assert.rejects(f.generations.snapshot(cancelled.signal));
  const live = f.generations.snapshot();
  await until(() => f.watches.length === 1);
  cancelled.abort();
  await gone;
  assert.equal(f.watches.length, 1);
  assert.equal(f.watches[0].stopped, false);
  f.watches[0].publish(snapshot('one'));
  assert.equal((await live).currentRevision, 'one');
  f.generations.close();
});

test('one consumer callback failing does not destroy other consumers or the validated route', async () => {
  const f = fixture(), abort = new AbortController();
  const refused = assert.rejects(f.generations.follow(() => { throw new Error('consumer refused'); }, abort.signal), /consumer refused/);
  const current = f.generations.snapshot();
  await until(() => f.watches.length === 1);
  f.watches[0].publish(snapshot('a'));
  await refused;
  assert.equal((await current).currentRevision, 'a');
  assert.equal(f.watches[0].stopped, false);
  assert.equal(f.failed.length, 0);
  f.generations.close();
});

test('repeated resets join the cancelled source before admitting one replacement watch', async () => {
  const finishes: (() => void)[] = [];
  const publications: ((snapshot: RuntimeGenerationSnapshot) => void | Promise<void>)[] = [];
  const signals: AbortSignal[] = [];
  let active = 0, maximum = 0;
  const generations = new RuntimeGenerations(() => ({
    watchGenerations(publish, {signal} = {}) {
      assert.ok(signal);
      signals.push(signal);
      publications.push(publish);
      active += 1;
      maximum = Math.max(maximum, active);
      // Cancellation requests shutdown, but the native source still has work to join.
      return new Promise<void>(resolve => {
        let ended = false;
        finishes.push(() => {
          if (ended) return;
          ended = true;
          active -= 1;
          resolve();
        });
      });
    },
  }), () => undefined, () => undefined);
  try {
    const initial = assert.rejects(generations.snapshot(), /verification source changed/);
    await until(() => finishes.length === 1);
    generations.reset();
    await initial;
    for (let count = 0; count < 20; count++) {
      const cancelled = assert.rejects(generations.snapshot(), /verification source changed/);
      generations.reset();
      await cancelled;
    }
    const latest = generations.snapshot();
    assert.equal(finishes.length, 1);
    assert.equal(active, 1);
    assert.equal(signals[0].aborted, true);
    finishes[0]();
    await until(() => finishes.length === 2);
    const next = snapshot('successor');
    await publications[1](next);
    assert.equal(await latest, next);
    assert.equal(maximum, 1);
  } finally {
    generations.close();
    for (const finish of finishes) finish();
  }
  assert.equal(active, 0);
});
