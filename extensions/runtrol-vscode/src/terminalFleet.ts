import type { TerminalDescriptor, TerminalIndexSnapshot } from "./runtimeTypes";
import type { RuntimeGenerationSnapshot, ValidatedLocator } from "@runtrol/runtime-client";

const FLEET_RETRY_MS = 15_000;

/// The hosted terminals of every Runtime generation, read as one list.
///
/// A conversation's terminal lives in the exact generation that opened it, and an update leaves that generation
/// draining beside the new one for as long as its conversations run. Each generation
/// publishes only its own terminals. A window that followed one generation therefore saw none of the others,
/// and the provider's own process roster still showed those conversations alive, so the sidebar took them for
/// terminals somebody else owned and refused to open them (measured 2026-08-29: five draining generations held
/// eight idle conversations, and every one of their rows was a dead end). One snapshot per generation, merged,
/// lets a row find its terminal whichever generation owns it and attach there instead of resuming a copy.
export class TerminalFleet {
  private readonly byGeneration = new Map<string, TerminalIndexSnapshot>();
  private readonly unreachable = new Map<string, string>();

  /// One validated source drives every generation. Retiring streams retain their admission slot
  /// until cancellation completes; a burst replaces only the latest desired snapshot.
  async followGenerations(
    followListed: (
      receive: (snapshot: RuntimeGenerationSnapshot) => void,
      signal: AbortSignal,
    ) => Promise<void>,
    followGeneration: (
      generation: ValidatedLocator,
      receive: (snapshot: TerminalIndexSnapshot) => void,
      signal: AbortSignal,
    ) => Promise<void>,
    publish: () => void,
    signal: AbortSignal,
  ): Promise<void> {
    if (signal.aborted) return;
    type Worker = {
      generation: ValidatedLocator;
      abort: AbortController;
      run: Promise<void> | null;
      retryAt: number;
    };
    const following = new Map<string, Worker>();
    const lifetime = new AbortController();
    const sourceState: { latest: RuntimeGenerationSnapshot | null } = { latest: null };
    let wake = () => {};
    let sourceError: unknown;
    let sourceFailed = false;
    const stop = () => { lifetime.abort(); wake(); };
    const fail = (error: unknown) => {
      if (lifetime.signal.aborted) return;
      sourceFailed = true;
      sourceError = error;
      stop();
    };
    signal.addEventListener("abort", stop, { once: true });
    const wanted = (worker: Worker) => sourceState.latest?.generations.some(
      (entry) => entry.digest === worker.generation.digest && entry.revision === worker.generation.revision,
    ) ?? false;
    const owns = (worker: Worker) => !lifetime.signal.aborted && !worker.abort.signal.aborted
      && following.get(worker.generation.digest) === worker && wanted(worker);
    const source = Promise.resolve().then(async () => {
      if (lifetime.signal.aborted) return;
      await followListed((snapshot) => {
        if (lifetime.signal.aborted) return;
        sourceState.latest = snapshot;
        wake();
      }, lifetime.signal);
      if (!lifetime.signal.aborted) throw new Error("Runtime generation observation stopped unexpectedly");
    }).catch(fail);
    try {
      while (!lifetime.signal.aborted) {
        const now = Date.now();
        const ready = new Promise<void>((resolve) => { wake = resolve; });
        let changed = false;
        for (const [digest, worker] of following) {
          if (wanted(worker) && !worker.abort.signal.aborted) continue;
          if (!worker.abort.signal.aborted) {
            worker.abort.abort();
            this.delete(digest);
            changed = true;
          }
          if (!worker.run) following.delete(digest);
        }
        if (changed) publish();
        let running = [...following.values()].filter((worker) => worker.run !== null).length;
        for (const generation of sourceState.latest?.generations ?? []) {
          let worker = following.get(generation.digest);
          if (worker?.run || worker?.abort.signal.aborted || (worker?.retryAt ?? 0) > now) continue;
          if (running >= (sourceState.latest?.generations.length ?? 0)) break;
          worker = { generation, abort: new AbortController(), run: null, retryAt: 0 };
          following.set(generation.digest, worker);
          const current = worker;
          running += 1;
          current.run = Promise.resolve().then(async () => {
            if (!owns(current)) return;
            await followGeneration(generation, (snapshot) => {
              if (!owns(current)) return;
              this.set(generation.digest, snapshot);
              publish();
            }, current.abort.signal);
            if (owns(current)) throw new Error("Runtime terminal observation stopped unexpectedly");
          }).catch((error: unknown) => {
            if (!owns(current)) return;
            this.markUnreachable(generation.digest, error instanceof Error ? error.message : String(error));
            current.retryAt = Date.now() + FLEET_RETRY_MS;
            publish();
          }).finally(() => {
            current.run = null;
            wake();
          }).catch(fail);
        }
        const retries = [...following.values()].filter((worker) =>
          !worker.run && !worker.abort.signal.aborted && wanted(worker) && worker.retryAt > now,
        );
        const retry = retries.length ? setTimeout(() => wake(),
          Math.max(0, Math.min(...retries.map((worker) => worker.retryAt)) - Date.now())) : undefined;
        try { await ready; } finally { clearTimeout(retry); }
      }
    } finally {
      stop();
      signal.removeEventListener("abort", stop);
      for (const worker of following.values()) worker.abort.abort();
      await Promise.all([source, ...[...following.values()].map((worker) => worker.run)]);
      for (const digest of following.keys()) this.delete(digest);
      if (following.size) publish();
    }
    if (sourceFailed) throw sourceError;
  }

  /// The latest snapshot one generation pushed.
  set(generation: string, snapshot: TerminalIndexSnapshot): void {
    this.byGeneration.set(generation, snapshot);
    this.unreachable.delete(generation);
  }

  /// A generation that ended, or is no longer listed, contributes nothing.
  delete(generation: string): void {
    this.byGeneration.delete(generation);
    this.unreachable.delete(generation);
  }

  /// A listed generation this window could not follow. Its terminals are unknown rather than absent, and the
  /// merged snapshot says so instead of quietly listing fewer conversations than the machine runs.
  markUnreachable(generation: string, why: string): void {
    this.byGeneration.delete(generation);
    this.unreachable.set(generation, why);
  }

  /// One snapshot for the sidebar. Generations are laid out in digest order, so the same fleet always reads the
  /// same way whichever generation happened to answer first.
  merged(): TerminalIndexSnapshot {
    const terminals: TerminalDescriptor[] = [];
    const warnings: string[] = [];
    for (const generation of [...this.byGeneration.keys()].sort(compare)) {
      const snapshot = this.byGeneration.get(generation);
      if (!snapshot) continue;
      terminals.push(...snapshot.terminals);
      warnings.push(...snapshot.warnings);
    }
    for (const generation of [...this.unreachable.keys()].sort(compare)) {
      warnings.push(`Runtime generation ${generation} could not be followed: ${this.unreachable.get(generation) ?? ""}`);
    }
    return { terminals, warnings };
  }
}

function compare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
