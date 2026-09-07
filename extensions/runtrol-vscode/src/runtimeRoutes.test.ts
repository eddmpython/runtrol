import assert from 'node:assert/strict';
import {test} from 'node:test';
import {RuntimeRoutes, type RuntimeRoute, type ReadyRuntimeRoute, type RuntimeRouteCandidate} from './runtimeRoutes';
import type {RuntimeGenerationSnapshot} from '@runtrol/runtime-client';
import {validatedLocator as validatedLocatorForTesting} from '../../../clients/typescript/src/testing';

const deferred = () => {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return {promise, resolve};
};
async function until(check: () => boolean) {
  for (let n = 0; n < 100; n++) {
    if (check()) return;
    await new Promise(resolve => setImmediate(resolve));
  }
  throw new Error('bounded test deadline');
}
function snapshot(revision: string): RuntimeGenerationSnapshot {
  const locator = validatedLocatorForTesting('home', 'pipe-' + revision, '0.1.1', revision.padEnd(64, '0'));
  return {current: {state: 'running', locator}, currentRevision: revision, generations: [locator]};
}
type MockCommand = {id: string; closed: number; close(): void};
type MockWindow = {id: string; active: boolean; closed: boolean};
type ReadyRoute = ReadyRuntimeRoute<MockCommand, MockWindow>;
type Candidate = RuntimeRouteCandidate<MockCommand, MockWindow>;
function fixture(prepare?: (route: RuntimeRoute, signal: AbortSignal, candidate: Candidate, index: number) => Promise<Candidate>) {
  const connections: MockCommand[] = [], windows: MockWindow[] = [], publications: (ReadyRoute | null)[] = [], failures: unknown[] = [];
  let calls = 0;
  const routes = new RuntimeRoutes<MockCommand, MockWindow>({
    async prepare(route, signal) {
      const index = ++calls;
      const command = {id: route.currentRevision + ':' + index, closed: 0, close() { this.closed++; }};
      connections.push(command);
      const window = {id: command.id, active: false, closed: false};
      windows.push(window);
      const candidate = {
        command,
        commit() { window.active = true; return window; },
        abort() { command.close(); window.closed = true; },
      };
      if (prepare) return prepare(route, signal, candidate, index);
      return candidate;
    },
    changed: route => publications.push(route),
    failed: error => failures.push(error),
  });
  return {routes, connections, windows, publications, failures, get calls() { return calls; }};
}

test('simultaneous new-work acquisitions share one prepared route', async () => {
  const gate = deferred();
  const f = fixture(async (_route, _signal, candidate) => { await gate.promise; return candidate; });
  try {
    f.routes.observe(snapshot('a'));
    const actions = Array.from({length: 100}, () => f.routes.run(async route => route.command.id));
    await until(() => f.calls === 1);
    gate.resolve();
    assert.deepEqual(new Set(await Promise.all(actions)), new Set(['a:1']));
    for (let n = 0; n < 100; n++) f.routes.observe(snapshot('a'));
    await f.routes.run(async () => undefined);
    assert.equal(f.calls, 1);
  } finally { f.routes.close(); }
  assert.equal(f.connections[0].closed, 1);
});

test('handover routes new work immediately after readiness while an old command and its window remain usable', async () => {
  const f = fixture(), held = deferred();
  let oldRoute: ReadyRoute | undefined;
  let executions = 0;
  try {
    f.routes.observe(snapshot('a'));
    const oldAction = f.routes.run(async route => {
      oldRoute = route;
      executions++;
      await held.promise;
      f.routes.invalidate(route);
      throw new Error('old transport failed after uncertain mutation');
    });
    const rejected = assert.rejects(oldAction, /uncertain mutation/);
    await until(() => oldRoute !== undefined);
    f.routes.observe(snapshot('b'));
    assert.equal(await f.routes.run(async route => route.command.id), 'b:2');
    assert.equal(f.connections[0].closed, 0, 'in-flight old work holds its command');
    assert.ok(oldRoute);
    assert.equal(oldRoute.window.closed, false, 'window ownership is not the command lifetime');
    held.resolve();
    await rejected;
    assert.equal(executions, 1, 'uncertain mutations never replay');
    assert.equal(f.connections[0].closed, 1);
    assert.equal(await f.routes.run(async route => route.command.id), 'b:2');
    assert.equal(f.calls, 2, 'old failure did not invalidate the successor');
  } finally { held.resolve(); f.routes.close(); }
});

test('obsolete preparation cannot publish or run a queued command', async () => {
  const gate = deferred();
  const f = fixture(async (_route, _signal, candidate, index) => {
    if (index === 1) await gate.promise;
    return candidate;
  });
  try {
    f.routes.observe(snapshot('a'));
    const command = f.routes.run(async route => route.command.id);
    await until(() => f.calls === 1);
    f.routes.observe(snapshot('b'));
    f.routes.observe(snapshot('c'));
    gate.resolve();
    assert.equal(await command, 'c:2');
    assert.equal(f.connections[0].closed, 1);
    assert.equal(f.windows[0].active, false);
    assert.deepEqual(f.publications.map(route => route?.currentRevision), ['c']);
    assert.equal(f.calls, 2, 'only the latest pending primary is prepared');
  } finally { gate.resolve(); f.routes.close(); }
});

test('a primary returning during cancelled preparation receives fresh readiness, not the aborted candidate', async () => {
  const gate = deferred();
  const f = fixture(async (_route, _signal, candidate, index) => {
    if (index === 1) await gate.promise;
    return candidate;
  });
  try {
    f.routes.observe(snapshot('a'));
    const command = f.routes.run(async route => route.command.id);
    await until(() => f.calls === 1);
    f.routes.observe(snapshot('b'));
    f.routes.observe(snapshot('a'));
    gate.resolve();
    assert.equal(await command, 'a:2');
    assert.equal(f.connections[0].closed, 1);
    assert.equal(f.windows[0].active, false);
    assert.equal(f.failures.length, 0);
  } finally { gate.resolve(); f.routes.close(); }
});

test('failed successor window preparation refuses new work and preserves the old ownership group', async () => {
  let unavailable = true;
  const f = fixture(async (route, _signal, candidate) => {
    if (route.currentRevision === 'b' && unavailable) {
      candidate.abort();
      throw new Error('window registration refused');
    }
    return candidate;
  });
  try {
    f.routes.observe(snapshot('a'));
    const old = await f.routes.run(async route => route);
    f.routes.observe(snapshot('b'));
    let executions = 0;
    await assert.rejects(f.routes.run(async () => { executions++; }), /registration refused/);
    assert.equal(executions, 0);
    assert.equal(old.window.closed, false);
    assert.equal(old.command.closed, 0);
    assert.equal(f.failures.length, 1);
    unavailable = false;
    assert.equal(await f.routes.run(async route => route.currentRevision), 'b');
    assert.equal(old.command.closed, 1);
    assert.equal(old.window.closed, false);
  } finally { f.routes.close(); }
});

test('observation failure removes cached authority for new commands without replaying or cancelling admitted work', async () => {
  const f = fixture(), gate = deferred();
  let admitted = false;
  try {
    f.routes.observe(snapshot('a'));
    const command = f.routes.run(async () => { admitted = true; await gate.promise; return 'once'; });
    await until(() => admitted);
    f.routes.observationFailed(new Error('locator ownership validation failed'));
    await assert.rejects(f.routes.run(async () => assert.fail('stale admission')), /ownership validation/);
    assert.equal(f.connections[0].closed, 0);
    gate.resolve();
    assert.equal(await command, 'once');
    assert.equal(f.connections[0].closed, 1);
  } finally { gate.resolve(); f.routes.close(); }
});

test('disposal rejects pending admission and closes a late candidate without publishing it', async () => {
  const gate = deferred();
  const f = fixture(async (_route, _signal, candidate) => { await gate.promise; return candidate; });
  f.routes.observe(snapshot('a'));
  const command = assert.rejects(f.routes.run(async () => assert.fail('disposed admission')));
  await until(() => f.calls === 1);
  f.routes.close();
  gate.resolve();
  await command;
  assert.equal(f.connections[0].closed, 1);
  assert.equal(f.windows[0].active, false);
  assert.deepEqual(f.publications, []);
});
