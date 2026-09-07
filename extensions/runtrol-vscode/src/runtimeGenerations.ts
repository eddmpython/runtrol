import type { RuntimeGenerationSnapshot, RuntimeLocator } from "@runtrol/runtime-client";

type GenerationListener = {
  snapshot(snapshot: RuntimeGenerationSnapshot): void;
  failed(error: unknown): void;
};

/** One OS observation belongs to the Studio lifetime. Consumers share its validated snapshots. */
export class RuntimeGenerations {
  private current: RuntimeGenerationSnapshot | null = null;
  private watching: AbortController | null = null;
  private work: Promise<void> | null = null;
  private restartRequested = false;
  private readonly listeners = new Set<GenerationListener>();
  private ended = false;

  constructor(
    private readonly locate: () => Pick<RuntimeLocator, "watchGenerations">,
    private readonly observed: (snapshot: RuntimeGenerationSnapshot) => void,
    private readonly failed: (error: unknown) => void,
  ) {}

  get latest(): RuntimeGenerationSnapshot | null { return this.current; }

  /** A fresh consumer restarts failed observation. Its existing recovery policy owns retry timing. */
  snapshot(signal?: AbortSignal): Promise<RuntimeGenerationSnapshot> {
    if (this.ended) return Promise.reject(new Error("Runtime generation observation ended"));
    if (signal?.aborted) return Promise.reject(signal.reason);
    if (this.current) return Promise.resolve(this.current);
    return new Promise((resolve, reject) => {
      const cleanup = (): void => {
        this.listeners.delete(listener);
        signal?.removeEventListener("abort", abort);
      };
      const listener: GenerationListener = {
        snapshot: value => { cleanup(); resolve(value); },
        failed: error => { cleanup(); reject(error); },
      };
      const abort = (): void => listener.failed(signal?.reason);
      this.listeners.add(listener);
      signal?.addEventListener("abort", abort, { once: true });
      this.start();
    });
  }

  /** Receivers consume immutable latest state synchronously; long work belongs in their own bounded lane. */
  follow(receive: (snapshot: RuntimeGenerationSnapshot) => void, signal: AbortSignal): Promise<void> {
    if (signal.aborted) return Promise.resolve();
    if (this.ended) return Promise.reject(new Error("Runtime generation observation ended"));
    return new Promise((resolve, reject) => {
      const cleanup = (): void => {
        this.listeners.delete(listener);
        signal.removeEventListener("abort", abort);
      };
      const listener: GenerationListener = {
        snapshot: value => {
          try { receive(value); } catch (error) { listener.failed(error); }
        },
        failed: error => { cleanup(); reject(error); },
      };
      const abort = (): void => { cleanup(); resolve(); };
      this.listeners.add(listener);
      signal.addEventListener("abort", abort, { once: true });
      if (this.current) listener.snapshot(this.current);
      if (this.listeners.has(listener)) this.start();
    });
  }

  /** The executable/digest source changed. No snapshot validated under the old source is reused. */
  reset(): void {
    if (this.ended) return;
    this.watching?.abort();
    this.watching = null;
    this.restartRequested = false;
    this.current = null;
    this.fail(new Error("Runtime locator verification source changed"));
  }

  close(): void {
    if (this.ended) return;
    this.ended = true;
    this.watching?.abort();
    this.watching = null;
    this.restartRequested = false;
    this.current = null;
    this.fail(new Error("Runtime generation observation ended"));
  }

  private start(): void {
    if (this.watching || this.ended) return;
    if (this.work) {
      this.restartRequested = true;
      return;
    }
    this.restartRequested = false;
    const watching = new AbortController();
    this.watching = watching;
    const observe = async (): Promise<void> => {
      try {
        if (this.watching !== watching || watching.signal.aborted) return;
        await this.locate().watchGenerations(snapshot => {
          if (this.watching !== watching || watching.signal.aborted) return;
          this.current = snapshot;
          this.observed(snapshot);
          for (const listener of [...this.listeners]) {
            if (this.listeners.has(listener)) listener.snapshot(snapshot);
          }
        }, {signal: watching.signal});
        if (!watching.signal.aborted) throw new Error("Runtime generation observation stopped unexpectedly");
      } catch (error) {
        if (this.watching !== watching || watching.signal.aborted) return;
        this.watching = null;
        this.current = null;
        this.fail(error);
      } finally {
        if (this.watching === watching) this.watching = null;
      }
    };
    // A cancelled watcher still owns its validation slot until its in-flight native work has joined.
    const work = Promise.resolve().then(observe);
    this.work = work;
    const settled = (): void => {
      if (this.work !== work) return;
      this.work = null;
      if (this.restartRequested && this.listeners.size > 0 && !this.ended) this.start();
    };
    void work.then(settled, error => { this.fail(error); settled(); });
  }

  private fail(error: unknown): void {
    this.failed(error);
    for (const listener of [...this.listeners]) {
      if (this.listeners.has(listener)) listener.failed(error);
    }
  }
}
