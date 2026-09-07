import type { AppScope, IntegrationGrant } from "@runtrol/runtime-client";

export const STUDIO_INPUT_PRIORITY: AppScope = "session.input.priority";
export const STUDIO_SCOPES: readonly AppScope[] = [
  "provider.read", "model.read", "session.list", "session.native.discover", "session.output.read",
  "session.start", "session.resume", "session.input.write", "session.stop", "approval.respond.low",
  "approval.respond.high", "session.delete", STUDIO_INPUT_PRIORITY,
];

interface ReviewStorage {
  get(key: string): PromiseLike<string | undefined>;
  store(key: string, value: string): PromiseLike<void>;
}

function reviewKey(grant: IntegrationGrant): string {
  return `runtrol.runtime.inputPriorityReview.${grant.integrationId}`;
}

export async function readStudioInputPriorityReview(storage: ReviewStorage, grant: IntegrationGrant): Promise<boolean> {
  return await storage.get(reviewKey(grant)) === "reviewed";
}

export async function persistStudioInputPriorityReview(storage: ReviewStorage, grant: IntegrationGrant): Promise<void> {
  // Old windows overwrite the shared identity snapshot. This monotonic, identity-bound decision has its own key.
  await storage.store(reviewKey(grant), "reviewed");
}

/// A capability upgrade is reviewed once for this identity. Owner narrowing is never an enrollment failure.
export async function reviewStudioInputPriority(
  supported: boolean,
  stored: { grant?: IntegrationGrant; inputPriorityReviewed?: true },
  rememberReview: () => Promise<void>,
  requestPriority: (grant: IntegrationGrant) => Promise<boolean>,
): Promise<boolean> {
  if (!supported || stored.inputPriorityReviewed || !stored.grant) return false;
  // Persist before administration: a lost reply or later owner revocation cannot trigger another elevation.
  await rememberReview();
  const grant = stored.grant;
  if (grant.scopes.includes(STUDIO_INPUT_PRIORITY)
    || !STUDIO_SCOPES.every((scope) => scope === STUDIO_INPUT_PRIORITY || grant.scopes.includes(scope))) return false;
  return requestPriority(grant);
}
