import assert from "node:assert/strict";
import { test } from "node:test";
import type { RuntimeClient, WindowClient, WindowRevealSubscription } from "@runtrol/runtime-client";
import { WindowPreparations } from "./windowPreparations";
import { WindowConnections, type WindowGroupCandidate, type WindowRoute, type WindowState } from "./windowConnections";
import { validatedLocator as validatedLocatorForTesting } from "../../../clients/typescript/src/testing";
function deferred() {
    let resolve!: () => void, reject!: (error: unknown) => void;
    const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
}
const tick = () => new Promise<void>(resolve => setImmediate(resolve));
async function until(check: () => unknown) {
    for (let count = 0; count < 100; count++) {
        if (check())
            return;
        await tick();
    }
    throw new Error("fixture did not settle");
}
function route(tag: string): WindowRoute {
    const locator = validatedLocatorForTesting("home", `pipe-${tag}`, "1", tag.repeat(64));
    return { locator, currentRevision: locator.revision };
}
const state: WindowState = { register: { windowSessionId: "window", hostGeneration: "host", vscodeVersion: "fixture", workspaceFolders: [] }, update: { terminals: [] } };
function stream(): Pick<WindowRevealSubscription, "next" | "close"> {
    const waits: {
        reject(error: unknown): void;
    }[] = [];
    let closed = false;
    return {
        next() {
            if (closed)
                return Promise.reject(new Error("closed"));
            return new Promise((_resolve, reject) => { waits.push({ reject }); });
        },
        close() { closed = true; for (const wait of waits.splice(0))
            wait.reject(new Error("closed")); },
    };
}
type Lane = "registration" | "mirror" | "input" | "reveal";
type Connection = {
    route: WindowRoute;
    lane: Lane;
    closed: boolean;
    close(): void;
    windows(): WindowClient;
    initialization: RuntimeClient["initialization"];
};
type Controls = {
    connect: ((route: WindowRoute, signal: AbortSignal) => Promise<void>) | null;
    register: ((route: WindowRoute) => Promise<void>) | null;
    candidate: ((route: WindowRoute, candidate: WindowGroupCandidate<Connection>) => Promise<void>) | null;
};
function fixture() {
    const a = route("a"), b = route("b"), listed = [a.locator, b.locator], lifetime = new AbortController();
    const connections: Connection[] = [], calls: WindowRoute[] = [], signals: AbortSignal[] = [];
    let registrations = 0, commits = 0, aborts = 0;
    const controls: Controls = { connect: null, register: null, candidate: null };
    function connection(r: WindowRoute, lane: Lane): Connection {
        const reveals = stream();
        const surface = {
            register: async () => { registrations++; await controls.register?.(r); return { registrationGeneration: registrations, ownerToken: "" }; },
            update: async () => { },
            watchReveals: async () => reveals as WindowRevealSubscription,
        } satisfies Pick<WindowClient, "register" | "update" | "watchReveals">;
        // Only these typed SDK methods and the capability projection are used by this fixture.
        const c: Connection = { route: r, lane, closed: false, initialization: { serverCapabilities: {} } as RuntimeClient["initialization"],
            close() { this.closed = true; reveals.close(); }, windows: () => surface as unknown as WindowClient };
        connections.push(c);
        return c;
    }
    const groups = new WindowConnections({ listed: () => listed, connect: async (r, lane) => connection(r, lane), authorityRevision: () => "grant", connectionFailure: () => true, reveal: () => { }, failed: () => { } }, lifetime.signal);
    const preparations = new WindowPreparations({ listed: () => listed, lookup: revision => groups.lookup(revision), prepare: async (r, signal) => {
            calls.push(r);
            signals.push(signal);
            await controls.connect?.(r, signal);
            const c = connection(r, "registration"), candidate = await groups.prepare(r, c, state, signal);
            await controls.candidate?.(r, candidate);
            return { update: next => candidate.update(next), commit() { commits++; return candidate.commit(); }, abort() { aborts++; candidate.abort(); } };
        } }, lifetime.signal);
    return { a, b, listed, lifetime, connections, calls, signals, controls, groups, preparations,
        get registrations() { return registrations; }, get commits() { return commits; }, get aborts() { return aborts; },
        close() { preparations.close(); groups.close(); assert.ok(connections.every(c => c.closed)); } };
}
test('route preparation shares connection before register and blocks ensure until owner commit', async () => {
    const f = fixture(), connecting = deferred(), registering = deferred();
    try {
        f.controls.connect = async () => connecting.promise;
        f.controls.register = async () => registering.promise;
        const route = f.preparations.prepareRoute(f.a);
        await until(() => f.calls.length === 1);
        let ready = false;
        const ensure = f.preparations.ensure(f.a).then(g => { ready = true; return g; });
        assert.equal(f.connections.length, 0);
        assert.equal(f.calls.length, 1);
        connecting.resolve();
        await until(() => f.registrations === 1);
        await tick();
        assert.equal(ready, false);
        assert.equal(f.calls.length, 1);
        registering.resolve();
        const candidate = await route;
        await tick();
        assert.equal(ready, false);
        assert.equal(f.commits, 0);
        await candidate.update(state);
        const group = candidate.commit();
        assert.equal(await ensure, group);
        assert.equal(f.commits, 1);
        assert.throws(() => candidate.commit(), /no longer/);
        candidate.abort();
        assert.equal(group.closed, false);
    }
    finally {
        connecting.resolve();
        registering.resolve();
        f.close();
    }
});
test('initial ensure auto commits once and competing primary only reuses the committed group', async () => {
    const f = fixture(), gate = deferred();
    try {
        f.controls.register = async () => gate.promise;
        const ensure = f.preparations.ensure(f.a);
        await until(() => f.registrations === 1);
        let routeReady = false;
        const primary = f.preparations.prepareRoute(f.a).then(c => { routeReady = true; return c; });
        await tick();
        assert.equal(routeReady, false);
        gate.resolve();
        const group = await ensure, candidate = await primary;
        assert.equal(candidate.commit(), group);
        assert.equal(f.commits, 1);
        assert.equal(f.registrations, 1);
        assert.throws(() => candidate.commit(), /no longer/);
    }
    finally {
        gate.resolve();
        f.close();
    }
});
test('a second route caller cannot prematurely commit or abort the first route candidate', async () => {
    const f = fixture();
    try {
        const first = await f.preparations.prepareRoute(f.a);
        let arrived = false;
        const second = f.preparations.prepareRoute(f.a).then(c => { arrived = true; return c; });
        await tick();
        assert.equal(arrived, false);
        const group = first.commit();
        const reused = await second;
        reused.abort();
        assert.equal(group.closed, false);
        assert.equal(f.commits, 1);
    }
    finally {
        f.close();
    }
});
test('candidate abort rejects ready waiters, cancels its signal and permits a fresh same-revision attempt', async () => {
    const f = fixture();
    try {
        const candidate = await f.preparations.prepareRoute(f.a);
        const waiting = f.preparations.ensure(f.a);
        const refused = assert.rejects(waiting, /aborted/);
        candidate.abort();
        await refused;
        assert.equal(f.signals[0].aborted, true);
        assert.equal(f.groups.lookup(f.a.currentRevision), undefined);
        const next = await f.preparations.ensure(f.a);
        assert.equal(next.closed, false);
        assert.equal(f.registrations, 2);
        assert.equal(f.commits, 1);
    }
    finally {
        f.close();
    }
});
test('clear during connect separates same-revision reauthentication and cleans a late old connection', async () => {
    const f = fixture(), gate = deferred();
    try {
        f.controls.connect = async () => gate.promise;
        const old = f.preparations.prepareRoute(f.a);
        await until(() => f.calls.length === 1);
        const oldWaiter = f.preparations.ensure(f.a), oldRefused = assert.rejects(old, /new authority/), waiterRefused = assert.rejects(oldWaiter, /new authority/);
        f.preparations.clear(Error('new authority'));
        f.groups.clear('new authority');
        assert.equal(f.signals[0].aborted, true);
        f.controls.connect = null;
        const next = await f.preparations.ensure(f.a);
        await oldRefused;
        await waiterRefused;
        gate.resolve();
        await tick();
        await tick();
        assert.equal(f.groups.lookup(f.a.currentRevision), next);
        assert.equal(next.closed, false);
        assert.equal(f.calls.length, 2);
        assert.equal(f.connections.filter(c => c.lane === 'registration' && !c.closed).length, 1);
    }
    finally {
        gate.resolve();
        f.close();
    }
});
test('clear after candidate creation cleans late candidate without deleting a newer same-revision record', async () => {
    const f = fixture(), oldGate = deferred(), newGate = deferred();
    try {
        f.controls.candidate = async () => oldGate.promise;
        const old = f.preparations.prepareRoute(f.a);
        await until(() => f.registrations === 1 && f.connections.length === 2);
        const refused = assert.rejects(old, /reauth/);
        f.preparations.clear(Error('reauth'));
        f.groups.clear('reauth');
        f.controls.candidate = null;
        f.controls.register = async () => newGate.promise;
        const next = f.preparations.ensure(f.a);
        await until(() => f.registrations === 2);
        oldGate.resolve();
        await refused;
        await tick();
        assert.equal(f.aborts, 1);
        const joining = f.preparations.ensure(f.a);
        assert.equal(f.calls.length, 2);
        newGate.resolve();
        assert.equal(await joining, await next);
        assert.equal(f.commits, 1);
    }
    finally {
        oldGate.resolve();
        newGate.resolve();
        f.close();
    }
});
test('joining caller cancellation does not cancel the producer or another waiter', async () => {
    const f = fixture(), gate = deferred(), abort = new AbortController();
    try {
        f.controls.register = async () => gate.promise;
        const primary = f.preparations.prepareRoute(f.a);
        await until(() => f.registrations === 1);
        const joined = f.preparations.ensure(f.a, abort.signal), refused = assert.rejects(joined, /only waiter/), other = f.preparations.ensure(f.a);
        abort.abort(Error('only waiter'));
        await refused;
        assert.equal(f.signals[0].aborted, false);
        gate.resolve();
        const candidate = await primary, group = candidate.commit();
        assert.equal(await other, group);
    }
    finally {
        gate.resolve();
        f.close();
    }
});
test('initial producer cancellation aborts unfinished connection and rejects every dependent waiter promptly', async () => {
    const f = fixture(), gate = deferred(), abort = new AbortController();
    try {
        f.controls.connect = async () => gate.promise;
        const first = f.preparations.ensure(f.a, abort.signal);
        await until(() => f.calls.length === 1);
        const other = f.preparations.prepareRoute(f.a), firstRefused = assert.rejects(first, /producer stop/), otherRefused = assert.rejects(other, /producer stop/);
        abort.abort(Error('producer stop'));
        await firstRefused;
        await otherRefused;
        assert.equal(f.signals[0].aborted, true);
        gate.resolve();
        await tick();
        assert.equal(f.registrations, 0);
    }
    finally {
        gate.resolve();
        f.close();
    }
});
test('membership pruning and lifetime close bound pending records without owning committed groups', async () => {
    const f = fixture(), gate = deferred();
    try {
        const existing = await f.preparations.ensure(f.a);
        f.controls.connect = async () => gate.promise;
        const pending = f.preparations.prepareRoute(f.b), refused = assert.rejects(pending, /membership/);
        await until(() => f.calls.length === 2);
        f.listed.splice(1);
        f.preparations.pruneMembership();
        await refused;
        await assert.rejects(f.preparations.ensure(f.b), /validated generation/);
        f.preparations.close();
        assert.equal(existing.closed, false);
        await assert.rejects(f.preparations.ensure(f.a), /lifetime ended/);
        gate.resolve();
        await tick();
    }
    finally {
        gate.resolve();
        f.close();
    }
});
test('late old connection failure cannot erase the new same-revision preparation', async () => {
    const f = fixture(), oldGate = deferred(), newGate = deferred();
    try {
        f.controls.connect = async () => oldGate.promise;
        const old = f.preparations.ensure(f.a);
        await until(() => f.calls.length === 1);
        const refused = assert.rejects(old, /replace/);
        f.preparations.clear(Error('replace'));
        f.groups.clear('replace');
        f.controls.connect = null;
        f.controls.register = async () => newGate.promise;
        const next = f.preparations.prepareRoute(f.a);
        await until(() => f.registrations === 1);
        oldGate.reject(Error('late old connect failure'));
        await refused;
        await tick();
        let ready = false;
        const joined = f.preparations.ensure(f.a).then(group => { ready = true; return group; });
        newGate.resolve();
        const candidate = await next;
        await tick();
        assert.equal(ready, false);
        assert.equal(f.calls.length, 2);
        const group = candidate.commit();
        assert.equal(await joined, group);
    }
    finally {
        newGate.resolve();
        f.close();
    }
});
test('producer abort after candidate readiness refuses dependent ready waiters and prevents commit', async () => {
    const f = fixture(), abort = new AbortController();
    try {
        const candidate = await f.preparations.prepareRoute(f.a, abort.signal);
        const waiting = f.preparations.ensure(f.a), refused = assert.rejects(waiting, /owner changed/);
        abort.abort(Error('owner changed'));
        await refused;
        assert.throws(() => candidate.commit(), /no longer/);
        assert.equal(f.commits, 0);
        assert.equal(f.groups.lookup(f.a.currentRevision), undefined);
    }
    finally {
        f.close();
    }
});
test('membership loss at candidate commit rejects ready waiters without committing a stale group', async () => {
    const f = fixture();
    try {
        const candidate = await f.preparations.prepareRoute(f.a);
        const waiting = f.preparations.ensure(f.a), refused = assert.rejects(waiting, /validated generation/);
        f.listed.splice(0, 1);
        assert.throws(() => candidate.commit(), /validated generation/);
        await refused;
        assert.equal(f.commits, 0);
        assert.equal(f.groups.lookup(f.a.currentRevision), undefined);
        await assert.rejects(f.preparations.ensure({ ...f.b, currentRevision: 'mismatch' }), /validated generation/);
        assert.equal(f.calls.length, 1);
    }
    finally {
        f.close();
    }
});
test('lifetime abort during preparation rejects promptly and closes a late connection', async () => {
    const f = fixture(), gate = deferred();
    try {
        f.controls.connect = async () => gate.promise;
        const pending = f.preparations.prepareRoute(f.a);
        await until(() => f.calls.length === 1);
        const refused = assert.rejects(pending, /lifetime ended/);
        f.lifetime.abort();
        await refused;
        assert.equal(f.signals[0].aborted, true);
        gate.resolve();
        await tick();
        assert.equal(f.registrations, 0);
        await assert.rejects(f.preparations.ensure(f.a), /lifetime ended/);
    }
    finally {
        gate.resolve();
        f.close();
    }
});
