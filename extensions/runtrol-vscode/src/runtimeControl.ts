import { RuntimeRequestError, type ControlLease } from "@runtrol/runtime-client";

export type CachedControlAction = "acquire" | "renew" | "reuse";

export type StoredControlState = {
  readonly runtimeInstanceId: string;
  readonly leases: readonly ControlLease[];
};

export function settleControlPersistence(
  persistence: Promise<void>,
  inlineMs: number,
): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error?: unknown): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (error === undefined) resolve();
      else reject(error);
    };
    const timeout = setTimeout(finish, inlineMs);
    persistence.then(
      () => finish(),
      (error: unknown) => finish(error),
    );
  });
}

export function cachedControlAction(
  current: ControlLease | undefined,
  nowMs: number,
): CachedControlAction {
  if (!current || current.expiresAtMs <= nowMs) return "acquire";
  if (current.expiresAtMs <= nowMs + 5_000) return "renew";
  return "reuse";
}

export function sessionDisappearedAfterCool(error: unknown): boolean {
  return error instanceof RuntimeRequestError && error.failure.code === "sessionNotFound";
}

export function restorableControls(
  state: StoredControlState | undefined,
  runtimeInstanceId: string,
  nowMs: number,
): readonly ControlLease[] {
  if (state?.runtimeInstanceId !== runtimeInstanceId) return [];
  return state.leases.filter((lease) => lease.expiresAtMs > nowMs);
}
