export const HOST_LIFETIME_MS: number;
export const HOST_DEADLINE_ENV: string;
export function hostDeadline(value: string | undefined): number | null;
export function readJourneyStep<T>(coordination: string, name: string, idleTimeoutMs: number,
  deadlineAtMs?: number | null): Promise<T | { readonly kind: "done" }>;
