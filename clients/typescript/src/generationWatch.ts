/** Internal scheduling only. Unvalidated routing hints never leave this module. */
export function watchValidatedGenerations<T>(
  arm: (changed: () => void, failed: (error: unknown) => void) => () => void,
  hint: (signal: AbortSignal) => Promise<string>,
  inspect: (signal: AbortSignal) => Promise<{ readonly value: T; readonly fingerprint: string }>,
  publish: (value: T) => void | Promise<void>,
  signal?: AbortSignal,
): Promise<void> {
  if (signal?.aborted) return Promise.resolve();
  return new Promise<void>((resolve, reject) => {
    let active = true;
    let epoch = 1;
    let consumed = 0;
    let running = false;
    let armed = false;
    let failure: unknown;
    const inspection = new AbortController();
    let accepted: string | null = null;
    let close: (() => void) | undefined;
    const settle = (): void => {
      if (active || running || !armed) return;
      if (failure === undefined) resolve(); else reject(failure);
    };
    const finish = (error?: unknown): void => {
      if (!active) return;
      active = false;
      failure = error;
      inspection.abort();
      signal?.removeEventListener("abort", abort);
      close?.();
      settle();
    };
    const abort = (): void => finish();
    const pump = async (): Promise<void> => {
      if (running || !active) return;
      running = true;
      try {
        while (active && consumed !== epoch) {
          const inspectingEpoch = epoch;
          // A malformed hint only requests validation; it never authorizes a result or hides an error.
          const observed = await hint(inspection.signal).catch(() => null);
          if (!active) break;
          if (inspectingEpoch !== epoch) continue;
          if (accepted !== null && observed === accepted) {
            consumed = inspectingEpoch;
            continue;
          }
          const checked = await inspect(inspection.signal);
          if (!active) break;
          if (inspectingEpoch !== epoch) continue;
          consumed = inspectingEpoch;
          const changed = accepted !== checked.fingerprint;
          accepted = checked.fingerprint;
          if (changed) await publish(checked.value);
        }
      } catch (error: unknown) {
        finish(error);
      } finally {
        running = false;
        settle();
      }
    };
    signal?.addEventListener("abort", abort, { once: true });
    try {
      // Arm before the initial inspection. An event while inspecting advances the epoch instead of being lost.
      close = arm(() => { epoch += 1; void pump(); }, finish);
      armed = true;
      if (!active) close();
      else void pump();
      settle();
    } catch (error: unknown) {
      armed = true;
      finish(error);
      settle();
    }
  });
}
