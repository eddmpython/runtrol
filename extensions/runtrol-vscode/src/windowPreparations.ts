import type { ValidatedLocator } from "@runtrol/runtime-client";
import type {
  WindowConnection, WindowGroupCandidate, WindowOwnershipGroup, WindowRoute,
} from "./windowConnections.js";

export interface WindowPreparationHooks<C extends WindowConnection> {
  listed(): readonly ValidatedLocator[];
  lookup(revision: string): WindowOwnershipGroup<C> | undefined;
  /** Own connection creation from its first await; the caller supplies current authority and state. */
  prepare(route: WindowRoute, signal: AbortSignal): Promise<WindowGroupCandidate<C>>;
}

type Record<C extends WindowConnection> = {
  readonly route: WindowRoute;
  readonly controller: AbortController;
  readonly prepared: Promise<WindowGroupCandidate<C>>;
  readonly ready: Promise<WindowOwnershipGroup<C>>;
  resolve(group: WindowOwnershipGroup<C>): void;
  reject(error: unknown): void;
  detach(): void;
  candidate?: WindowGroupCandidate<C>;
  settled: boolean;
};

/** In-flight preparation only. WindowConnections remains the sole committed group owner. */
export class WindowPreparations<C extends WindowConnection> {
  private readonly pending = new Map<string, Record<C>>();
  private closed = false;
  private readonly abort: () => void;

  constructor(private readonly hooks: WindowPreparationHooks<C>, private readonly lifetime: AbortSignal) {
    this.abort = () => this.close();
    lifetime.addEventListener("abort", this.abort, { once: true });
    if (lifetime.aborted) this.close();
  }

  async prepareRoute(route: WindowRoute, signal?: AbortSignal): Promise<WindowGroupCandidate<C>> {
    this.admit(route, signal);
    const pending = this.pending.get(route.currentRevision);
    if (pending) return this.reuse(await waitFor(pending.ready, signal));
    const existing = this.hooks.lookup(route.currentRevision);
    if (existing) return this.reuse(existing);
    const record = this.start(route, signal);
    await waitFor(record.prepared, record.controller.signal);
    return {
      update: state => {
        this.requirePending(record);
        return record.candidate!.update(state);
      },
      commit: () => this.commit(record),
      abort: () => this.cancel(record, new Error("window route preparation aborted")),
    };
  }

  async ensure(route: WindowRoute, signal?: AbortSignal): Promise<WindowOwnershipGroup<C>> {
    this.admit(route, signal);
    const pending = this.pending.get(route.currentRevision);
    if (pending) return waitFor(pending.ready, signal);
    const existing = this.hooks.lookup(route.currentRevision);
    if (existing) return existing;
    const record = this.start(route, signal);
    // Only this initial ensure owns the commit; later ensure/route callers wait for ready.
    void record.prepared.then(() => {
      if (!record.settled) this.commit(record);
    }).catch(error => this.cancel(record, error));
    return waitFor(record.ready, signal);
  }

  pruneMembership(): void {
    const listed = this.validatedMembership();
    for (const record of [...this.pending.values()]) {
      if (!listed.some(locator => locator.revision === record.route.currentRevision)) {
        this.cancel(record, new Error("window preparation incarnation left the validated membership"));
      }
    }
  }

  clear(reason: unknown): void {
    // Remove old records synchronously so reauthentication cannot join an old authority's promise.
    for (const record of [...this.pending.values()]) this.cancel(record, reason);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.lifetime.removeEventListener("abort", this.abort);
    this.clear(new Error("window preparation lifetime ended"));
  }

  private start(route: WindowRoute, signal?: AbortSignal): Record<C> {
    this.pruneMembership();
    if (this.pending.size >= new Set(this.validatedMembership().map(locator => locator.revision)).size) {
      throw new Error("window preparations exceed the validated generation membership");
    }
    const controller = new AbortController();
    let resolve!: Record<C>["resolve"];
    let reject!: Record<C>["reject"];
    const ready = new Promise<WindowOwnershipGroup<C>>((yes, no) => { resolve = yes; reject = no; });
    // A route candidate may have no ready waiters when aborted.
    void ready.catch(() => undefined);
    let record!: Record<C>;
    const prepared = Promise.resolve().then(async () => {
      controller.signal.throwIfAborted();
      const candidate = await this.hooks.prepare(route, controller.signal);
      if (record.settled || controller.signal.aborted) {
        candidate.abort();
        controller.signal.throwIfAborted();
        throw new Error("window preparation ended before its candidate arrived");
      }
      record.candidate = candidate;
      return candidate;
    });
    const cancel = () => this.cancel(record, signal?.reason ?? new Error("window preparation cancelled"));
    record = { route, controller, prepared, ready, resolve, reject, settled: false,
      detach: () => signal?.removeEventListener("abort", cancel) };
    this.pending.set(route.currentRevision, record);
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    void prepared.catch(error => this.cancel(record, error));
    return record;
  }

  private commit(record: Record<C>): WindowOwnershipGroup<C> {
    this.requirePending(record);
    try {
      this.admit(record.route);
      const group = record.candidate!.commit();
      record.settled = true;
      record.detach();
      if (this.pending.get(record.route.currentRevision) === record) this.pending.delete(record.route.currentRevision);
      record.resolve(group);
      return group;
    } catch (error) { this.cancel(record, error); throw error; }
  }

  private cancel(record: Record<C>, reason: unknown): void {
    if (record.settled) return;
    record.settled = true;
    record.detach();
    if (this.pending.get(record.route.currentRevision) === record) this.pending.delete(record.route.currentRevision);
    record.controller.abort(reason);
    record.candidate?.abort();
    record.reject(reason);
  }

  private reuse(group: WindowOwnershipGroup<C>): WindowGroupCandidate<C> {
    let settled = false;
    const requireReady = () => {
      if (settled || group.closed || this.closed) throw new Error("window group candidate is no longer available");
    };
    return {
      update: state => { requireReady(); return group.update(state); },
      commit: () => { requireReady(); settled = true; return group; },
      abort: () => { settled = true; },
    };
  }

  private requirePending(record: Record<C>): void {
    if (record.settled || this.pending.get(record.route.currentRevision) !== record || !record.candidate) {
      throw new Error("window preparation candidate is no longer available");
    }
  }

  private validatedMembership(): readonly ValidatedLocator[] {
    const listed = this.hooks.listed();
    for (const locator of listed) locator.assertSdkValidated();
    return listed;
  }

  private admit(route: WindowRoute, signal?: AbortSignal): void {
    if (this.closed) throw new Error("window preparation lifetime ended");
    signal?.throwIfAborted();
    route.locator.assertSdkValidated();
    if (route.currentRevision !== route.locator.revision
      || !this.validatedMembership().some(locator => locator.revision === route.currentRevision)) {
      throw new Error("window preparation route is not a validated generation member");
    }
  }
}

function waitFor<T>(promise: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise<T>((resolve, reject) => {
    const abort = () => { signal.removeEventListener("abort", abort); reject(signal.reason); };
    signal.addEventListener("abort", abort, { once: true });
    void promise.then(value => { signal.removeEventListener("abort", abort); resolve(value); },
      error => { signal.removeEventListener("abort", abort); reject(error); });
  });
}
