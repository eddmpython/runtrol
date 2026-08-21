import type { Conversation } from "./conversationList";
import type { ProviderCapabilities } from "./runtimeTypes";

/// What "delete" means for one row, decided before anything is asked or done.
///
/// Two different acts hide under one word, and the surface must never guess which. A conversation Runtrol
/// supervises is closed and forgotten here (the pointer is Runtrol's, the history stays the provider's). A
/// conversation only the provider holds is deleted by the provider, through its own surface, and only where
/// that surface exists: a service that publishes no way to delete what it stored is told apart up front, in
/// its own words, instead of after a click that could not have worked.
export type ConversationDeletion =
  | { readonly kind: "forgetSupervised" }
  | { readonly kind: "deleteNative"; readonly serviceName: string }
  | { readonly kind: "unsupported"; readonly why: string };

export function conversationDeletion(
  row: Conversation,
  capabilities: ProviderCapabilities | null,
): ConversationDeletion {
  if (row.session) return { kind: "forgetSupervised" };
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
    detail: `${row.serviceName} removes it from its own history. Runtrol keeps no copy to restore it from.`,
    button: `Delete from ${row.serviceName}`,
  };
}
