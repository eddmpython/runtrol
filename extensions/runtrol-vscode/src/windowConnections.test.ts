import assert from "node:assert/strict";
import { test } from "node:test";
import type { RuntimeClient, WindowClient, WindowInputSubscription, WindowRevealSubscription, WindowRegisterParams, WindowUpdateParams, WindowMirrorOutputParams, WindowMirrorOpenParams, } from "@runtrol/runtime-client";
import { WindowConnections, type WindowConnectionHooks, type WindowRoute, type WindowState } from "./windowConnections";
import { validatedLocator as validatedLocatorForTesting } from "../../../clients/typescript/src/testing";
function later() {
    let resolve!: () => void;
    let reject!: (error: unknown) => void;
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
const state = (tag = "initial"): WindowState => ({
    register: { windowSessionId: "window", hostGeneration: "host", vscodeVersion: "fixture", workspaceFolders: [] },
    update: { terminals: [{ terminalKey: tag, name: tag, shellIntegration: false }] },
});
const mirrorRequest = (key = "shell"): Omit<WindowMirrorOpenParams, "registrationGeneration" | "ownerToken"> => ({
    windowSessionId: "window", terminalKey: key, executionId: "execution", providerId: "provider",
    commandLine: "fixture", cwd: "C:/owned", geometry: { columns: 80, rows: 24 },
});
function route(tag: string, revision = `revision-${tag}`, draining = false): WindowRoute {
    return { locator: validatedLocatorForTesting("home", `pipe-${tag}`, "1", tag.repeat(64), draining, `pipe-${tag}-control`, revision), currentRevision: revision };
}
type RevealEvent = Awaited<ReturnType<WindowRevealSubscription["next"]>>;
type InputEvent = Awaited<ReturnType<WindowInputSubscription["next"]>>;
class Stream<T> {
    private readonly waiters: {
        resolve(value: T): void;
        reject(error: unknown): void;
    }[] = [];
    private readonly items: T[] = [];
    closed = false;
    next(): Promise<T> {
        if (this.closed)
            return Promise.reject(new Error("closed"));
        if (this.items.length)
            return Promise.resolve(this.items.shift()!);
        return new Promise<T>((resolve, reject) => { this.waiters.push({ resolve, reject }); });
    }
    send(item: T): void { const waiter = this.waiters.shift(); if (waiter)
        waiter.resolve(item);
    else
        this.items.push(item); }
    close(): void { if (this.closed)
        return; this.closed = true; for (const waiter of this.waiters.splice(0))
        waiter.reject(new Error("closed")); this.items.length = 0; }
}
type Lane = "registration" | "mirror" | "input" | "reveal";
type FixtureConnection = {
    route: WindowRoute;
    lane: Lane;
    closed: boolean;
    authority: string;
    initialization: RuntimeClient["initialization"];
    reveal: Stream<RevealEvent>;
    input: Stream<InputEvent>;
    close(): void;
    windows(): WindowClient;
};
type FixtureEvent = {
    route: string;
    lane: Lane;
    method: string;
    params: unknown;
};
type Controls = {
    register: ((route: WindowRoute, lane: Lane, params: WindowRegisterParams) => Promise<void>) | null;
    update: ((route: WindowRoute, lane: Lane, params: WindowUpdateParams) => Promise<void>) | null;
    mirrorOutput: ((route: WindowRoute, lane: Lane, params: WindowMirrorOutputParams) => Promise<void>) | null;
    mirrorOpen: ((route: WindowRoute, lane: Lane, params: WindowMirrorOpenParams) => Promise<void>) | null;
    connect: ((route: WindowRoute, lane: Exclude<Lane, "registration">, signal: AbortSignal) => Promise<FixtureConnection>) | null;
    authority: string;
    folders?: boolean;
};
function updateOf(event: FixtureEvent): WindowUpdateParams { assert.equal(event.method, "update"); return event.params as WindowUpdateParams; }
function fixture() {
    const a = route("a"), b = route("b"), c = route("c"), all = [a, b, c];
    const connections: FixtureConnection[] = [], events: FixtureEvent[] = [];
    const failures: {
        g: unknown;
        lane: Lane;
        error: unknown;
    }[] = [], reveals: [
        string,
        string
    ][] = [], inputs: [
        string,
        InputEvent
    ][] = [];
    const lifetime = new AbortController();
    const controls: Controls = { register: null, update: null, mirrorOutput: null, mirrorOpen: null, connect: null, authority: "grant-1" };
    function connection(r: WindowRoute, lane: Lane): FixtureConnection {
        const reveal = new Stream<RevealEvent>(), input = new Stream<InputEvent>();
        const record = (method: string, params: unknown) => events.push({ route: r.currentRevision, lane, method, params });
        const surface = {
            register: async (p: WindowRegisterParams) => { record("register", p); await controls.register?.(r, lane, p); return { registrationGeneration: 1, ownerToken: `private-${r.currentRevision}` }; },
            update: async (p: WindowUpdateParams) => { record("update", p); await controls.update?.(r, lane, p); },
            watchReveals: async () => reveal as unknown as WindowRevealSubscription,
            watchInput: async (p: Parameters<WindowClient["watchInput"]>[0]) => { record("input", p); return input as unknown as WindowInputSubscription; },
            mirrorOpen: async (p: WindowMirrorOpenParams) => { record("mirrorOpen", p); await controls.mirrorOpen?.(r, lane, p); return { terminalId: `terminal-${r.currentRevision}-${p.terminalKey}` }; },
            mirrorOutput: async (p: WindowMirrorOutputParams) => { record("output", p); await controls.mirrorOutput?.(r, lane, p); },
            mirrorEnd: async (p: Parameters<WindowClient["mirrorEnd"]>[0]) => { record("end", p); },
        } satisfies Pick<WindowClient, "register" | "update" | "watchReveals" | "watchInput" | "mirrorOpen" | "mirrorOutput" | "mirrorEnd">;
        // The SDK classes have private transport state. Only their typed public wire methods cross this fixture boundary.
        const initialization = { serverCapabilities: controls.folders === undefined ? {} : { windowWorkspaceFoldersUpdate: controls.folders } } as RuntimeClient["initialization"];
        const client: FixtureConnection = { initialization, route: r, lane, closed: false, authority: controls.authority, reveal, input,
            close() { this.closed = true; reveal.close(); input.close(); }, windows: () => surface as unknown as WindowClient };
        connections.push(client);
        return client;
    }
    const hooks: WindowConnectionHooks<FixtureConnection> = {
        listed: () => all.map(value => value.locator),
        connect: async (r, lane, signal) => controls.connect ? controls.connect(r, lane, signal) : connection(r, lane),
        authorityRevision: client => client.authority,
        connectionFailure: error => error instanceof Error && "transport" in error && error.transport === true,
        failed: (g, lane, error) => { failures.push({ g, lane, error }); },
        reveal: async (g, key) => { reveals.push([g.route.currentRevision, key]); },
        input: async (g, sub, signal) => { while (!signal.aborted) {
            const event = await sub.next();
            if (event.kind === "ended")
                return;
            inputs.push([g.route.currentRevision, event]);
        } },
    };
    const manager = new WindowConnections(hooks, lifetime.signal);
    const prepare = (r: WindowRoute) => manager.prepare(r, connection(r, "registration"), state());
    const commit = async (r: WindowRoute) => (await prepare(r)).commit();
    return { a, b, c, all, connections, events, failures, reveals, inputs, controls, lifetime, manager, connection, prepare, commit,
        close() { manager.close(); assert.ok(connections.every(client => client.closed), "all owned connections closed"); } };
}
test('normal handoff retains old mirror, proof, input and reveal on exact route', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), mirror = await old.openMirror(mirrorRequest());
        const next = await f.commit(f.b);
        await mirror.output('c3ludGhldGlj');
        await f.manager.update(state('latest'));
        assert.equal(old.closed, false);
        assert.equal(next.closed, false);
        assert.equal(mirror.route.currentRevision, f.a.currentRevision);
        assert.equal(f.events.filter(x => x.method === 'output')[0].route, f.a.currentRevision);
        for (const g of [old, next])
            assert.ok(f.events.some(x => x.route === g.route.currentRevision && x.method === 'update' && updateOf(x).terminals[0]!.terminalKey === 'latest'));
        f.connections.find(x => x.route === f.a && x.lane === 'reveal')!.reveal.send({ kind: 'requested', requested: { subscriptionId: 'fixture', terminalKey: 'shell' } });
        f.connections.find(x => x.route === f.a && x.lane === 'input')!.input.send({ kind: 'offered', offered: { sequence: 1, subscriptionId: 'fixture', binding: { windowSessionId: 'window', registrationGeneration: 1, hostGeneration: 'host', terminalKey: 'shell', executionId: 'execution', terminalId: 'terminal', processId: 1 } } });
        await until(() => f.reveals.length && f.inputs.length);
        assert.deepEqual(f.reveals, [[f.a.currentRevision, 'shell']]);
        assert.equal(f.inputs[0][0], f.a.currentRevision);
        const fresh = await next.openMirror(mirrorRequest('new'));
        await fresh.output('b25jZQ==');
        await mirror.end();
        assert.equal(f.events.filter(x => x.method === 'end')[0].route, f.a.currentRevision);
    }
    finally {
        f.close();
    }
});
test('same revision reuse and peer-only observation do not re-register; candidate abort preserves old group', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a);
        assert.equal(f.manager.lookup(f.a.currentRevision), old);
        const reused = await f.manager.prepare(f.a, old.connection, state());
        await reused.update(state('reuse-latest'));
        reused.abort();
        assert.equal(reused.commit(), old);
        assert.equal(f.events.filter(x => x.method === 'register').length, 1);
        const candidate = await f.prepare(f.b);
        candidate.abort();
        assert.equal(old.closed, false);
        assert.equal(f.manager.lookup(f.b.currentRevision), undefined);
        assert.ok(f.connections.filter(x => x.route === f.b).every(x => x.closed));
    }
    finally {
        f.close();
    }
});
test('late failed old update cannot delete or close the committed successor', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), gate = later();
        f.controls.update = async (r) => { if (r === f.a)
            await gate.promise; };
        const pending = old.update(state('pending'));
        await tick();
        const next = await f.commit(f.b);
        gate.reject(Object.assign(Error('old transport failed'), { transport: true }));
        await assert.rejects(pending, /old transport/);
        assert.equal(old.closed, true);
        assert.equal(next.closed, false);
        assert.equal(f.manager.lookup(f.b.currentRevision), next);
    }
    finally {
        f.close();
    }
});
test('latest update during candidate preparation reaches the candidate before commit', async () => {
    const f = fixture();
    try {
        const gate = later();
        f.controls.register = async (r) => { if (r === f.b)
            await gate.promise; };
        const preparing = f.prepare(f.b);
        await tick();
        await f.manager.update(state('newest'));
        gate.resolve();
        const candidate = await preparing;
        await candidate.update(state('commit-latest'));
        const group = candidate.commit();
        assert.equal(f.events.filter(x => x.route === f.b.currentRevision && x.method === 'update').map(updateOf).at(-1)!.terminals[0]!.terminalKey, 'commit-latest');
        assert.equal(group.closed, false);
    }
    finally {
        f.close();
    }
});
test('cancel while a connection opens closes late connection and preserves older owner', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), gate = later(), abort = new AbortController();
        f.controls.connect = async (r, lane) => { if (r === f.b)
            await gate.promise; return f.connection(r, lane); };
        const prepare = f.manager.prepare(f.b, f.connection(f.b, 'registration'), state(), abort.signal);
        await until(() => f.events.some(x => x.route === f.b.currentRevision && x.method === 'register'));
        abort.abort();
        gate.resolve();
        await assert.rejects(prepare);
        assert.equal(old.closed, false);
        assert.ok(f.connections.filter(x => x.route === f.b).every(x => x.closed));
    }
    finally {
        f.close();
    }
});
test('outcome-unknown mirror output is not replayed; feeder loss never migrates existing handle', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), mirror = await old.openMirror(mirrorRequest());
        await f.commit(f.b);
        f.controls.mirrorOutput = async () => { throw Object.assign(Error('outcomeUnknown'), { code: 'outcomeUnknown' }); };
        await assert.rejects(mirror.output('b25jZQ=='), /outcomeUnknown/);
        assert.equal(mirror.closed, true);
        await assert.rejects(mirror.output('YWdhaW4='));
        assert.equal(f.events.filter(x => x.method === 'output').length, 1);
        assert.equal(old.closed, false, 'feeder failure does not remove registration');
        f.controls.mirrorOutput = null;
        const newExecution = await old.openMirror(mirrorRequest('explicit-new'));
        await newExecution.output('bmV3');
        assert.equal(newExecution.route.currentRevision, f.a.currentRevision, 'explicit group handle never follows current route');
    }
    finally {
        f.close();
    }
});
test('authority change and input end fail closed without re-registering or rebinding mirror', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a);
        f.controls.authority = 'grant-2';
        await assert.rejects(old.openMirror(mirrorRequest()), /authority changed/);
        assert.equal(f.events.filter(x => x.method === 'mirrorOpen').length, 0);
        assert.equal(f.events.filter(x => x.method === 'register').length, 1);
        f.connections.find(x => x.route === f.a && x.lane === 'input')!.input.send({ kind: 'ended', ended: { subscriptionId: 'fixture', reason: 'authorityChanged' } });
        await until(() => f.failures.some(x => x.lane === 'input'));
        assert.equal(f.events.filter(x => x.method === 'register').length, 1);
        assert.equal(old.closed, false);
    }
    finally {
        f.close();
    }
});
test('answered update refusal preserves registration; late failure of closed same-digest incarnation cannot remove replacement', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a);
        f.controls.update = async () => { throw Error('scope denied'); };
        await assert.rejects(old.update(state('refused')));
        assert.equal(old.closed, false);
        f.controls.update = null;
        old.close('selected incarnation ended');
        const replacement = route('a', 'new-incarnation');
        f.all[0] = replacement;
        const next = await f.commit(replacement);
        old.registrationFailed(Object.assign(Error('late'), { transport: true }));
        assert.equal(f.manager.lookup(replacement.currentRevision), next);
        assert.equal(next.closed, false);
    }
    finally {
        f.close();
    }
});
test('unlisted group and replacement without retirement are refused, lifetime closes all retained groups', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a);
        await assert.rejects(f.prepare(route('d')), /not in the validated/);
        const replacement = route('a', 'replacement');
        f.all[0] = replacement;
        await assert.rejects(f.prepare(replacement), /retire the prior/);
        assert.equal(old.closed, false);
        await f.commit(f.b);
        f.lifetime.abort();
        assert.equal(old.closed, true);
        assert.ok(f.connections.every(x => x.closed));
    }
    finally {
        f.close();
    }
});
test('peer incarnation pruning preserves draining role changes but removes same-digest restarted owners', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), mirror = await old.openMirror(mirrorRequest());
        const primary = await f.commit(f.b);
        f.all[0] = route('a', f.a.currentRevision, true);
        f.manager.pruneMembership();
        assert.equal(old.closed, false);
        assert.equal(mirror.closed, false);
        f.all[0] = route('a', 'peer-restarted', true);
        f.manager.pruneMembership();
        assert.equal(old.closed, true);
        assert.equal(mirror.closed, true);
        assert.equal(primary.closed, false);
        assert.equal(f.manager.lookup(f.a.currentRevision), undefined);
        assert.equal(f.manager.lookup(f.b.currentRevision), primary);
    }
    finally {
        f.close();
    }
});
test('explicit authority recovery clears current groups and permits fresh registration without reviving old mirrors', async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a), mirror = await old.openMirror(mirrorRequest());
        const gate = later();
        f.controls.register = async (r) => { if (r === f.b)
            await gate.promise; };
        const preparing = f.prepare(f.b);
        await tick();
        f.manager.clear('explicit integration replacement');
        assert.equal(old.closed, true);
        assert.equal(mirror.closed, true);
        f.controls.authority = 'grant-2';
        const replacement = await f.commit(f.a);
        gate.resolve();
        await assert.rejects(preparing, /ended/);
        assert.notEqual(replacement, old);
        assert.equal(f.manager.lookup(f.a.currentRevision), replacement);
        old.registrationFailed(Object.assign(Error('late old failure'), { transport: true }));
        assert.equal(replacement.closed, false);
        await assert.rejects(mirror.output('b2xk'), /no longer available/);
        const fresh = await replacement.openMirror(mirrorRequest('fresh'));
        await fresh.output('bmV3');
        assert.equal(f.events.filter(x => x.method === 'output').length, 1);
        f.manager.close();
        await assert.rejects(f.prepare(f.a), /lifetime ended/);
    }
    finally {
        f.close();
    }
});
test("folder updates follow each exact registration capability and preserve owner proof", async () => {
    const f = fixture();
    try {
        const old = await f.commit(f.a);
        f.controls.folders = false;
        const disabled = await f.commit(f.b);
        f.controls.folders = true;
        const capable = await f.commit(f.c);
        const initial = state("latest");
        const next: WindowState = { ...initial, register: { ...initial.register, workspaceFolders: ["C:/owned", "C:/second"] } };
        const registrations = [old.registration, disabled.registration, capable.registration];
        await f.manager.update(next);
        const latest = (revision: string) => f.events.filter(event => event.route === revision && event.method === "update").map(updateOf).at(-1)!;
        assert.equal(Object.hasOwn(latest(f.a.currentRevision), "workspaceFolders"), false);
        assert.equal(Object.hasOwn(latest(f.b.currentRevision), "workspaceFolders"), false);
        assert.deepEqual(latest(f.c.currentRevision).workspaceFolders, ["C:/owned", "C:/second"]);
        await capable.update(state("empty"));
        assert.deepEqual(latest(f.c.currentRevision).workspaceFolders, []);
        for (const group of [old, disabled, capable]) {
            assert.equal(group.closed, false);
            assert.equal(group.registration, registrations[[old, disabled, capable].indexOf(group)]);
        }
        assert.equal(f.events.filter(event => event.method === "register").length, 3);
    }
    finally {
        f.close();
    }
});
