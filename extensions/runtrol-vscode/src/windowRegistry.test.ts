import assert from "node:assert/strict";
import { test } from "node:test";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { runInNewContext } from "node:vm";
import type * as vscode from "vscode";
import type { WindowInputSubscription } from "@runtrol/runtime-client";
import type { WindowRegistry as Registry, WindowGroupHandle, WindowSnapshot, WindowOwner, WindowPublisher, BoundMirrorHandle } from "./windowRegistry";
class EventEmitter<T> {
    private readonly listeners = new Set<(value: T) => void>();
    readonly event = (listener: (value: T) => void) => { this.listeners.add(listener); return { dispose: () => { this.listeners.delete(listener); } }; };
    fire(value: T): void { for (const listener of this.listeners)
        listener(value); }
    dispose(): void { this.listeners.clear(); }
}
const events = {
    open: new EventEmitter<vscode.Terminal>(), close: new EventEmitter<vscode.Terminal>(),
    integration: new EventEmitter<vscode.TerminalShellIntegrationChangeEvent>(),
    start: new EventEmitter<vscode.TerminalShellExecutionStartEvent>(), end: new EventEmitter<vscode.TerminalShellExecutionEndEvent>(),
    folders: new EventEmitter<vscode.WorkspaceFoldersChangeEvent>(),
};
const window = {
    terminals: [] as vscode.Terminal[], onDidOpenTerminal: events.open.event, onDidCloseTerminal: events.close.event,
    onDidChangeTerminalShellIntegration: events.integration.event,
    onDidStartTerminalShellExecution: events.start.event, onDidEndTerminalShellExecution: events.end.event,
};
const workspace = { workspaceFolders: [] as vscode.WorkspaceFolder[], onDidChangeWorkspaceFolders: events.folders.event };
// Load the actual adapter. Only VS Code's unavailable host surface is substituted; Node modules are delegated unchanged.
const requireFromTest = createRequire(__filename);
const { buildSync } = requireFromTest("esbuild") as typeof import("esbuild");
const extensionRoot = resolve(__dirname, "..");
const source = resolve(extensionRoot, "src/windowRegistry.ts");
const output = buildSync({ entryPoints: [source], bundle: true, platform: "node", format: "cjs", target: "node20", write: false,
    external: ["vscode"], alias: { "@runtrol/runtime-client": resolve(extensionRoot, "../../clients/typescript/src/index.ts") } });
const loaded: {
    exports: {
        WindowRegistry?: typeof Registry;
    };
} = { exports: {} };
runInNewContext(output.outputFiles[0].text, {
    module: loaded, exports: loaded.exports,
    require: (id: string) => id === "vscode" ? { window, workspace, env: { sessionId: "window" }, version: "fixture", EventEmitter } : requireFromTest(id),
    Buffer, process, console, setTimeout, clearTimeout, setInterval, clearInterval, setImmediate, clearImmediate, queueMicrotask, AbortController, AbortSignal,
}, { filename: source });
assert.ok(loaded.exports.WindowRegistry);
const WindowRegistry = loaded.exports.WindowRegistry;
const tick = () => new Promise<void>(done => setImmediate(done));
async function until(check: () => unknown) {
    for (let count = 0; count < 200; count++) {
        if (check())
            return;
        await tick();
    }
    throw new Error("fixture did not settle");
}
class Queue<T> implements AsyncIterableIterator<T> {
    private readonly values: T[] = [];
    private readonly waiters: ((value: IteratorResult<T>) => void)[] = [];
    closed = false;
    [Symbol.asyncIterator](): AsyncIterableIterator<T> { return this; }
    next(): Promise<IteratorResult<T>> {
        if (this.values.length)
            return Promise.resolve({ value: this.values.shift()!, done: false });
        if (this.closed)
            return Promise.resolve({ value: undefined, done: true });
        return new Promise(done => { this.waiters.push(done); });
    }
    push(value: T): void { const done = this.waiters.shift(); if (done)
        done({ value, done: false });
    else
        this.values.push(value); }
    end(): void { this.closed = true; for (const done of this.waiters.splice(0))
        done({ value: undefined, done: true }); }
}
type InputEvent = Awaited<ReturnType<WindowInputSubscription["next"]>>;
function channel() {
    const queue = new Queue<InputEvent>(), claims: number[] = [], receipts: Parameters<WindowInputSubscription["inputReceipt"]>[0][] = [];
    const surface = {
        next: async () => { const item = await queue.next(); if (item.done)
            throw new Error("closed"); return item.value; },
        close: () => queue.end(), claimInput: async (sequence: number) => { claims.push(sequence); return { text: "fixture\r" }; },
        inputReceipt: async (receipt: Parameters<WindowInputSubscription["inputReceipt"]>[0]) => { receipts.push(receipt); },
    } satisfies Pick<WindowInputSubscription, "next" | "close" | "claimInput" | "inputReceipt">;
    return { queue, claims, receipts, subscription: surface as unknown as WindowInputSubscription };
}
type Group = WindowGroupHandle & {
    id: string;
    abort: AbortController;
};
type FixtureTerminal = vscode.Terminal & {
    writes: {
        text: string;
        execute: boolean | undefined;
    }[];
    shown: number;
};
function fixture() {
    window.terminals = [];
    workspace.workspaceFolders = [];
    const outputs: {
        id: string;
        bytes: string;
    }[] = [], ends: {
        id: string;
        code?: number;
    }[] = [];
    const opens: {
        group: Group;
        handle: BoundMirrorHandle;
        params: Parameters<WindowGroupHandle["openMirror"]>[0];
    }[] = [];
    const changes: WindowSnapshot[] = [], reports: string[] = [], publishes: (WindowSnapshot & {
        primary: Group;
    })[] = [], groups: Group[] = [];
    const makeGroup = (id: string): Group => {
        const abort = new AbortController();
        const group: Group = { id, signal: abort.signal, abort, registration: { registrationGeneration: 1, ownerToken: `own-${id}` },
            async openMirror(params) {
                let closed = false;
                const handle = { terminalId: `${id}-${params.terminalKey}`, get closed() { return closed; },
                    async output(bytes: string) { if (abort.signal.aborted)
                        throw new Error("group ended"); outputs.push({ id, bytes }); },
                    async end(code?: number) { closed = true; ends.push({ id, code }); } } satisfies BoundMirrorHandle;
                opens.push({ group, handle, params });
                return handle;
            } };
        groups.push(group);
        return group;
    };
    const a = makeGroup("a"), b = makeGroup("b");
    let primary = a, owner: WindowOwner | null = null;
    const publisher: WindowPublisher = {
        setWindowOwner(value) { owner = value; return { dispose() { owner = null; } }; },
        providerCommandNames: async () => new Map([["provider", "provider"]]),
        publishWindow: async (snapshot) => { assert.ok(owner, "owner callbacks registered before first publish"); publishes.push({ ...snapshot, primary }); return primary; },
    };
    const registry = new WindowRegistry(publisher, value => reports.push(value), value => changes.push(value));
    const streams: Queue<string>[] = [];
    // VS Code owns Terminal and URI instances. This fixture supplies only the typed public surface read by the adapter.
    const terminalSurface = { name: "owned", creationOptions: {}, processId: Promise.resolve(1234),
        shellIntegration: { cwd: { fsPath: "C:/owned" } }, writes: [] as FixtureTerminal["writes"], shown: 0,
        sendText(text: string, execute?: boolean) { this.writes.push({ text, execute }); }, show() { this.shown++; } };
    const terminal = terminalSurface as unknown as FixtureTerminal;
    window.terminals = [terminal];
    registry.start();
    async function execute() {
        await until(() => registry.knownCommandNames() !== null && registry.currentState().update.terminals[0]?.processId === 1234);
        const openedCount = opens.length, queue = new Queue<string>();
        streams.push(queue);
        let reads = 0;
        const execution = { cwd: undefined, commandLine: { value: "provider", confidence: 2, isTrusted: true }, read() { reads++; return queue; } } satisfies vscode.TerminalShellExecution;
        events.start.fire({ terminal, execution, shellIntegration: terminal.shellIntegration! });
        assert.equal(reads, 1, "shell stream captured synchronously");
        await until(() => opens.length > openedCount);
        return { queue, execution };
    }
    return { a, b, registry, terminal, outputs, ends, opens, changes, reports, publishes, execute,
        get owner(): WindowOwner { assert.ok(owner); return owner; },
        setPrimary(group: Group) { primary = group; registry.resync(); },
        async close() { for (const queue of streams)
            queue.end(); registry.dispose(); assert.equal(owner, null); for (const group of groups)
            group.abort.abort(); await tick(); } };
}
test('actual WindowRegistry adapter pins output and end to returned group across a primary change', async () => {
    const f = fixture();
    try {
        const { queue, execution } = await f.execute();
        assert.equal(f.opens[0].group, f.a);
        f.setPrimary(f.b);
        await until(() => f.publishes.at(-1)!.primary === f.b);
        queue.push('own synthetic');
        await until(() => f.outputs.length === 1);
        assert.equal(f.outputs[0].id, 'a');
        events.end.fire({ terminal: f.terminal, execution, exitCode: 0, shellIntegration: f.terminal.shellIntegration! });
        queue.end();
        await until(() => f.ends.length === 1);
        assert.equal(f.ends[0].id, 'a');
        assert.ok(f.changes.length > 0);
        assert.equal(f.owner.state().register.windowSessionId, 'window');
    }
    finally {
        await f.close();
    }
});
test('registration-scoped dispatchers reject a colliding generation counter from another group', async () => {
    const f = fixture();
    const owner = channel(), other = channel();
    let ownerRun, otherRun;
    try {
        await f.execute();
        f.setPrimary(f.b);
        const state = f.registry.currentState(), row = state.update.terminals[0], opened = f.opens[0];
        ownerRun = f.registry.serveGroupInput(f.a, owner.subscription).catch(e => e);
        otherRun = f.registry.serveGroupInput(f.b, other.subscription).catch(e => e);
        const binding = { windowSessionId: 'window', registrationGeneration: 1, hostGeneration: state.register.hostGeneration, terminalKey: row.terminalKey, executionId: row.command!.executionId, terminalId: opened.handle.terminalId, processId: 1234 };
        const offered: InputEvent = { kind: 'offered', offered: { sequence: 1, subscriptionId: 'own', binding } };
        other.queue.push(offered);
        owner.queue.push(offered);
        await until(() => owner.receipts.length === 1 && other.receipts.length === 1);
        assert.equal(other.claims.length, 0);
        assert.equal(other.receipts[0].outcome, 'refused');
        assert.equal(owner.claims.length, 1);
        assert.deepEqual(f.terminal.writes, [{ text: 'fixture\r', execute: false }]);
        f.a.abort.abort();
        await ownerRun;
        assert.equal(f.registry.revealFromGroup(f.a, row.terminalKey), false);
        assert.equal(f.registry.revealFromGroup(f.b, row.terminalKey), true);
        assert.equal(f.terminal.shown, 1);
    }
    finally {
        await f.close();
        await ownerRun;
        await otherRun;
    }
});
test('terminal close removes owner input eligibility and ends the exact existing mirror', async () => {
    const f = fixture();
    const input = channel();
    let running;
    try {
        await f.execute();
        const state = f.registry.currentState(), row = state.update.terminals[0], opened = f.opens[0];
        running = f.registry.serveGroupInput(f.a, input.subscription).catch(e => e);
        events.close.fire(f.terminal);
        await until(() => f.ends.length === 1);
        assert.equal(f.ends[0].id, 'a');
        input.queue.push({ kind: 'offered', offered: { sequence: 1, subscriptionId: 'own', binding: { windowSessionId: 'window', registrationGeneration: 1, hostGeneration: state.register.hostGeneration, terminalKey: row.terminalKey, executionId: row.command!.executionId, terminalId: opened.handle.terminalId, processId: 1234 } } });
        await until(() => input.receipts.length === 1);
        assert.equal(input.claims.length, 0);
        assert.equal(f.terminal.writes.length, 0);
        assert.equal(f.registry.currentState().update.terminals.length, 0);
    }
    finally {
        await f.close();
        await running;
    }
});
test('explicit reauthentication creates a fresh dispatcher and never admits the retired execution binding', async () => {
    const f = fixture(), oldInput = channel(), freshInput = channel();
    let oldRun, freshRun;
    try {
        const oldExecution = await f.execute();
        const previous = f.opens[0], oldState = f.registry.currentState(), oldRow = oldState.update.terminals[0];
        const oldBinding = { windowSessionId: 'window', registrationGeneration: 1, hostGeneration: oldState.register.hostGeneration, terminalKey: oldRow.terminalKey, executionId: oldRow.command!.executionId, terminalId: previous.handle.terminalId, processId: 1234 };
        oldRun = f.owner.input(f.a, oldInput.subscription).catch(e => e);
        f.a.abort.abort();
        await oldRun;
        f.setPrimary(f.b);
        const refused = channel();
        await f.owner.input(f.a, refused.subscription);
        assert.equal(refused.queue.closed, true);
        freshRun = f.owner.input(f.b, freshInput.subscription).catch(e => e);
        freshInput.queue.push({ kind: 'offered', offered: { sequence: 1, subscriptionId: 'fresh', binding: oldBinding } });
        await until(() => freshInput.receipts.length === 1);
        assert.equal(freshInput.claims.length, 0);
        events.end.fire({ terminal: f.terminal, execution: oldExecution.execution, exitCode: 0, shellIntegration: f.terminal.shellIntegration! });
        oldExecution.queue.end();
        await f.execute();
        const current = f.registry.currentState().update.terminals[0], opened = f.opens.at(-1)!;
        assert.equal(opened.group, f.b);
        assert.notEqual(current.command!.executionId, oldRow.command!.executionId);
        freshInput.queue.push({ kind: 'offered', offered: { sequence: 2, subscriptionId: 'fresh', binding: { ...oldBinding, executionId: current.command!.executionId, terminalId: opened.handle.terminalId } } });
        await until(() => freshInput.receipts.length === 2);
        assert.equal(freshInput.claims.length, 1);
        assert.deepEqual(f.terminal.writes, [{ text: 'fixture\r', execute: false }]);
        assert.equal(f.registry.revealFromGroup(f.a, current.terminalKey), false);
        assert.equal(f.registry.revealFromGroup(f.b, current.terminalKey), true);
    }
    finally {
        await f.close();
        await oldRun;
        await freshRun;
    }
});
