export type WatchPriority = "foreground" | "background";

/// Coordinates the short opening phase of all session event watches.
///
/// Streams remain concurrent after their subscriptions start. Openings run one at a time, visible
/// conversations enter before queued sidebar activity watches. Superseded stream transports are synchronously
/// aborted by the Runtime SDK before a replacement is queued, so their later promise cleanup does not sit on the
/// visible switch path. Runtime bounded replay remains the only conversation memory.
export class WatchLifecycleGate {
  private active = false;
  private readonly foreground: PendingPermit[] = [];
  private readonly background: PendingPermit[] = [];

  acquire(priority: WatchPriority, signal: AbortSignal): Promise<(() => void) | null> {
    if (signal.aborted) return Promise.resolve(null);
    return new Promise((resolve) => {
      const queue = priority === "foreground" ? this.foreground : this.background;
      const pending: PendingPermit = {
        signal,
        resolve,
        abort: () => {
          const index = queue.indexOf(pending);
          if (index < 0) return;
          queue.splice(index, 1);
          resolve(null);
        },
      };
      signal.addEventListener("abort", pending.abort, { once: true });
      queue.push(pending);
      this.advance();
    });
  }

  private advance(): void {
    if (this.active) return;
    while (this.foreground.length > 0 || this.background.length > 0) {
      const pending = this.foreground.shift() ?? this.background.shift();
      if (!pending) return;
      pending.signal.removeEventListener("abort", pending.abort);
      if (pending.signal.aborted) {
        pending.resolve(null);
        continue;
      }
      this.active = true;
      let released = false;
      pending.resolve(() => {
        if (released) return;
        released = true;
        this.active = false;
        this.advance();
      });
      return;
    }
  }
}

type PendingPermit = {
  readonly signal: AbortSignal;
  readonly resolve: (release: (() => void) | null) => void;
  readonly abort: () => void;
};
