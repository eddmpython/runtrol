import type { Conversation } from "./conversationList";
import type { ProviderCapabilities } from "./runtimeTypes";

export type ConversationArchival =
  | { readonly kind: "archiveNative"; readonly serviceName: string }
  | { readonly kind: "unsupported"; readonly why: string };

export function conversationArchival(
  row: Conversation,
  capabilities: ProviderCapabilities | null,
): ConversationArchival {
  if (!row.native) {
    return { kind: "unsupported", why: `${row.title} has no provider-owned conversation to archive.` };
  }
  const capability = capabilities?.nativeSessionArchive;
  if (!capability) {
    return {
      kind: "unsupported",
      why: `${row.serviceName} has not said whether it can archive a stored conversation.`,
    };
  }
  if (capability.availability !== "available") {
    return {
      kind: "unsupported",
      why: capability.why
        ? `${row.serviceName} cannot archive this conversation: ${capability.why}`
        : `${row.serviceName} publishes no way to archive a stored conversation.`,
    };
  }
  return { kind: "archiveNative", serviceName: row.serviceName };
}

export function archivalQuestion(row: Conversation): { message: string; detail: string; button: string } {
  return {
    message: `Archive "${row.title}" in ${row.serviceName}?`,
    detail: `${row.session ? "Runtrol stops supervising it first. " : ""}`
      + `The conversation leaves this list and can be restored with ${row.serviceName}.`,
    button: `Archive in ${row.serviceName}`,
  };
}
