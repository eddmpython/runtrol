import { RuntimeRequestError, type ControlLease } from "@runtrol/runtime-client";

export type CachedControlAction = "acquire" | "renew" | "reuse";

export type StoredControlState = {
  readonly runtimeInstanceId: string;
  readonly leases: readonly ControlLease[];
};

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
