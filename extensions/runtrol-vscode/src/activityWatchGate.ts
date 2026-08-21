/// Serializes the connection handshake of background activity watches.
///
/// Windows named pipes keep a small ready pool. Opening every hot-session watch in the same event-loop turn
/// can occupy that pool long enough to delay the foreground conversation watch. The streams remain concurrent;
/// only the short connect-and-subscribe phase passes through this gate.
export class ActivityWatchGate {
  private active = false;
  private readonly waiting: PendingPermit[] = [];

  acquire(signal: AbortSignal): Promise<(() => void) | null> {
    if (signal.aborted) return Promise.resolve(null);
    return new Promise((resolve) => {
      const pending: PendingPermit = {
        signal,
        resolve,
        abort: () => {
          const index = this.waiting.indexOf(pending);
          if (index < 0) return;
          this.waiting.splice(index, 1);
          resolve(null);
        },
      };
      signal.addEventListener("abort", pending.abort, { once: true });
      this.waiting.push(pending);
      this.advance();
    });
  }

  private advance(): void {
    if (this.active) return;
    while (this.waiting.length > 0) {
      const pending = this.waiting.shift();
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
