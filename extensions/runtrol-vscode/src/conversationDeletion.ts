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

/// The question asked before a provider deletes a conversation. Modal, because the act is the provider's
/// and final; worded with the service's name, because it is the service's history that changes.
export function deletionQuestion(row: Conversation): { message: string; detail: string; button: string } {
  return {
    message: `Delete "${row.title}" from ${row.serviceName}?`,
    detail: `${row.session ? "Runtrol stops supervising it first. " : ""}`
      + `${row.serviceName} removes it from its own history. Runtrol keeps no copy to restore it from.`,
    button: `Delete from ${row.serviceName}`,
  };
}
