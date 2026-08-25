import type { Conversation } from "./conversationList";
import type { ProviderCapabilities } from "./runtimeTypes";

/// What "delete" means for one row, decided before anything is asked or done.
///
/// A provider-owned conversation is deleted by the provider even while Runtrol supervises it. The controller
/// closes that supervised pointer first. A session without a provider-owned identity only has its local
/// pointer forgotten.
export type ConversationDeletion =
  | { readonly kind: "forgetSupervised" }
  | { readonly kind: "deleteNative"; readonly serviceName: string }
  | { readonly kind: "unsupported"; readonly why: string };

/// Whether a row can be deleted at all, which is the one truth the inline affordance and the click share.
///
/// They used to decide it apart: the click asked `conversationDeletion`, the row's delete button keyed on a
/// native identity. An orphan pointer (supervised, but the service no longer lists it) is deletable by the
/// first and was invisible to the second, so two such rows sat with no delete button. One function now.
export function canDelete(row: Conversation, capabilities: ProviderCapabilities | null): boolean {
  return conversationDeletion(row, capabilities).kind !== "unsupported";
}

export function conversationDeletion(
  row: Conversation,
  capabilities: ProviderCapabilities | null,
): ConversationDeletion {
  if (!row.native && row.session) return { kind: "forgetSupervised" };
  if (!row.native) {
    return { kind: "unsupported", why: `${row.title} has no provider-owned conversation to delete.` };
  }
  const capability = capabilities?.nativeSessionDelete;
  if (!capability) {
    return {
      kind: "unsupported",
      why: `${row.serviceName} has not said whether it can delete a stored conversation.`,
    };
  }
  if (capability.availability !== "available") {
    return {
      kind: "unsupported",
      why: capability.why
        ? `${row.serviceName} cannot delete this conversation: ${capability.why}`
        : `${row.serviceName} publishes no way to delete a stored conversation.`,
    };
  }
  return { kind: "deleteNative", serviceName: row.serviceName };
}
