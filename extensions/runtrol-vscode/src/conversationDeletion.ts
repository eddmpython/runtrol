import type { Conversation } from "./conversationList";
import type { ProviderCapabilities } from "./runtimeTypes";

/// What "delete" means for one row, decided before anything is asked or done.
///
/// A provider-owned conversation is permanently deleted only after its original process has stopped. A session
/// without a provider-owned identity has no exact provider record this action is allowed to remove.
export type ConversationDeletion =
  | { readonly kind: "deleteNative"; readonly serviceName: string }
  | { readonly kind: "unsupported"; readonly why: string };

export type DeletionQuestion = {
  readonly message: string;
  readonly detail: string;
  readonly button: "Delete permanently";
};

/// Whether a row can be deleted at all, which is the one truth the inline affordance and the click share.
///
/// The click and row action must decide from this one function. In particular, a live process and an orphan
/// Runtrol pointer are close or stop actions, not permanent provider conversation deletion.
export function canDelete(row: Conversation, capabilities: ProviderCapabilities | null): boolean {
  return conversationDeletion(row, capabilities).kind !== "unsupported";
}

export function conversationDeletion(
  row: Conversation,
  capabilities: ProviderCapabilities | null,
): ConversationDeletion {
  if (row.live) {
    return {
      kind: "unsupported",
      why: `Stop ${row.title} before permanently deleting its provider-owned conversation.`,
    };
  }
  if (!row.native) {
    return {
      kind: "unsupported",
      why: `${row.title} has no exact provider-owned conversation to permanently delete. Close it in Runtrol instead.`,
    };
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

/// The irreversible confirmation shared by every Studio entry point that asks the operator.
export function deletionQuestion(row: Conversation, serviceName: string): DeletionQuestion {
  return {
    message: `Permanently delete ${row.title} from ${serviceName}?`,
    detail: "This removes the provider-owned conversation and its known related history records. Runtrol keeps no recovery copy.",
    button: "Delete permanently",
  };
}
