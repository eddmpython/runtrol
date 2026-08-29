/// Wait, unless told to stop waiting.
///
/// The delay resolves when the time is up or the moment the signal aborts, whichever comes first, and leaves
/// no timer behind either way. Every clock in this extension that a person can walk away from uses this one.
export function abortableDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    const timer = setTimeout(done, milliseconds);
    signal.addEventListener("abort", done, { once: true });
    function done(): void {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
  });
}
