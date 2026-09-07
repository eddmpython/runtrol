import type { RuntimeGenerationSnapshot, ValidatedLocator } from "@runtrol/runtime-client";

export type RuntimeRoute = { readonly locator: ValidatedLocator; readonly currentRevision: string };
export type ReadyRuntimeRoute<C, W> = RuntimeRoute & { readonly command: C; readonly window: W };
export type RuntimeRouteCandidate<C, W> = {
  readonly command: C;
  commit(): W;
  abort(): void;
};

export type RuntimeRouteHooks<C, W> = {
  prepare(route: RuntimeRoute, signal: AbortSignal): Promise<RuntimeRouteCandidate<C, W>>;
  changed(route: ReadyRuntimeRoute<C, W> | null): void;
  failed(error: unknown): void;
};

type HeldRoute<C, W> = {
  readonly value: ReadyRuntimeRoute<C, W>;
  users: number;
  retired: boolean;
};
type PendingRoute<C, W> = {
  readonly desired: RuntimeRoute;
  readonly abort: AbortController;
  readonly result: Promise<HeldRoute<C, W>>;
};

/** New work follows the latest validated primary. In-flight work keeps its exact connection. */
export class RuntimeRoutes<C extends { close(): void }, W> {
  private desired: RuntimeRoute | null = null;
  private ready: HeldRoute<C, W> | null = null;
  private pending: PendingRoute<C, W> | null = null;
  private readonly held = new Set<HeldRoute<C, W>>();
  private ended = false;
  private unavailable: unknown = new Error("no Runtime generation is available");

  constructor(private readonly hooks: RuntimeRouteHooks<C, W>) {}

  /** The SDK owns validation and selection. This class never reads a locator or selects a peer. */
  observe(snapshot: RuntimeGenerationSnapshot): void {
    if (this.ended) return;
    const current = snapshot.current;
    if (current.state !== "running" || snapshot.currentRevision === null) {
      this.unavailable = new Error("no Runtime generation is available for new work");
      this.select(null);
      return;
    }
    current.locator.assertSdkValidated();
    this.select({ locator: current.locator, currentRevision: snapshot.currentRevision });
  }

  /** Failed observation is not evidence that a cached primary is still selected. Existing views are separate. */
  observationFailed(error: unknown): void {
    if (this.ended) return;
    this.unavailable = error;
    this.select(null);
    this.hooks.failed(error);
  }

  /** The action is invoked once. A transport failure or selection change never replays a mutation. */
  async run<T>(action: (route: ReadyRuntimeRoute<C, W>) => Promise<T>): Promise<T> {
    while (true) {
      const route = await this.acquire();
      // Selection may have changed between acquire's resolution and this continuation.
      if (route !== this.ready || !this.isDesired(route.value)) continue;
      route.users += 1;
      try {
        return await action(route.value);
      } finally {
        route.users -= 1;
        this.release(route);
      }
    }
  }

  /** An obsolete operation may report failure after a successor committed. Only its own route is invalidated. */
  invalidate(route: ReadyRuntimeRoute<C, W>): void {
    const current = this.ready;
    if (!current || current.value !== route) return;
    this.ready = null;
    this.retire(current);
    this.hooks.changed(null);
  }

  close(): void {
    if (this.ended) return;
    this.ended = true;
    this.desired = null;
    this.pending?.abort.abort();
    this.ready = null;
    for (const route of this.held) {
      route.retired = true;
      // Disposal ends the Studio lifetime, including its outstanding command requests.
      route.value.command.close();
    }
    this.held.clear();
  }

  private select(next: RuntimeRoute | null): void {
    const unchanged = next?.currentRevision === this.desired?.currentRevision;
    this.desired = next;
    if (!unchanged) this.pending?.abort.abort();
    if (next === null) {
      if (this.ready) this.retire(this.ready);
      this.ready = null;
      if (!unchanged) this.hooks.changed(null);
      return;
    }
    // Preparation is coalesced with command acquisition and reports failure once in begin().
    void this.acquire().catch(() => undefined);
  }

  private async acquire(): Promise<HeldRoute<C, W>> {
    while (true) {
      if (this.ended) throw new Error("Runtime routing lifetime ended");
      const desired = this.desired;
      if (!desired) throw this.unavailable;
      if (this.ready && this.isDesired(this.ready.value)) return this.ready;
      const pending = this.pending ?? this.begin(desired);
      try {
        const route = await pending.result;
        if (route === this.ready && this.isDesired(route.value)) return route;
      } catch (error) {
        // A superseded preparation never ran the caller's action. Only selection acquisition may repeat.
        if (this.ended || (this.isDesired(pending.desired) && !pending.abort.signal.aborted)) throw error;
      }
    }
  }

  private begin(desired: RuntimeRoute): PendingRoute<C, W> {
    const abort = new AbortController();
    // Defer prepare until pending is installed, even when a test or adapter answers synchronously.
    const result = Promise.resolve().then(async () => {
      let candidate: RuntimeRouteCandidate<C, W> | null = null;
      try {
        abort.signal.throwIfAborted();
        candidate = await this.hooks.prepare(desired, abort.signal);
        abort.signal.throwIfAborted();
        if (this.ended || !this.isDesired(desired)) throw new Error("Runtime preparation was superseded");
        const window = candidate.commit();
        const route: HeldRoute<C, W> = {
          value: { ...desired, command: candidate.command, window }, users: 0, retired: false,
        };
        candidate = null;
        const old = this.ready;
        this.ready = route;
        this.held.add(route);
        if (old) this.retire(old);
        this.hooks.changed(route.value);
        return route;
      } catch (error) {
        candidate?.abort();
        if (!this.ended && this.isDesired(desired) && !abort.signal.aborted) this.hooks.failed(error);
        throw error;
      } finally {
        if (this.pending?.abort === abort) this.pending = null;
      }
    });
    const pending = { desired, abort, result };
    this.pending = pending;
    return pending;
  }

  private isDesired(route: RuntimeRoute): boolean {
    return this.desired?.currentRevision === route.currentRevision;
  }

  private retire(route: HeldRoute<C, W>): void {
    route.retired = true;
    this.release(route);
  }

  private release(route: HeldRoute<C, W>): void {
    if (!route.retired || route.users !== 0 || !this.held.delete(route)) return;
    route.value.command.close();
  }
}
