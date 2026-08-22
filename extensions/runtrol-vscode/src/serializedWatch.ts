/// One restartable asynchronous watch whose previous connection must finish before the next one starts.
///
/// Aborting a stream asks its transport to close. That close is asynchronous, so treating abort as immediate
/// completion can accumulate overlapping sockets while a person moves quickly between conversations.
export class SerializedWatch {
  private active: AbortController | null = null;
  private desired: { readonly generation: number; readonly key: string } | null = null;
  private generation = 0;
  private ready: Promise<void> = Promise.resolve();
  private resolveReady: () => void = () => {};
  private tail: Promise<void> = Promise.resolve();
  private disposed = false;

  get requested(): boolean {
    return this.desired !== null;
  }

  settled(): Promise<void> {
    return this.ready;
  }

  start(
    key: string,
    run: (signal: AbortSignal, ready: () => void) => Promise<void>,
  ): void {
    if (this.disposed || this.desired?.key === key) return;

    const generation = ++this.generation;
    this.desired = { generation, key };
    let resolveReady = () => {};
    this.ready = new Promise<void>((resolve) => {
      resolveReady = resolve;
    });
    this.resolveReady = resolveReady;

    const previous = this.tail;
    const current = previous.then(async () => {
      if (this.disposed || this.desired?.generation !== generation) {
        resolveReady();
        return;
      }
      const abort = new AbortController();
      this.active = abort;
      try {
        await run(abort.signal, resolveReady);
      } finally {
        resolveReady();
        if (this.active === abort) this.active = null;
        if (this.desired?.generation === generation) this.desired = null;
      }
    });
    // The owner reports watch failures inside `run`. Keeping a fulfilled tail makes a later restart possible
    // even if an unexpected failure escapes that boundary, without creating an unhandled rejection.
    this.tail = current.catch(() => undefined);
  }

  pause(): void {
    this.generation += 1;
    this.desired = null;
    this.active?.abort();
    this.resolveReady();
    this.resolveReady = () => {};
    this.ready = Promise.resolve();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.pause();
  }
}
