import { conversationDetail, type Conversation } from "./conversationList";

export type ConversationChoice = {
  label: string;
  description: string;
  detail: string;
  picked: boolean;
  conversation: Conversation;
};

/// The searchable form of the same list the sidebar shows.
///
/// It reads from the ordered rows rather than re-deriving them, so the two surfaces can never disagree about what
/// exists or what it is called.
export function conversationChoices(
  rows: readonly Conversation[],
  nowMs: number,
): ConversationChoice[] {
  return rows.map((conversation) => ({
    label: `${glyph(conversation)} ${conversation.title}`,
    description: conversationDetail(conversation, nowMs),
    detail: conversation.workspace,
    picked: conversation.open,
    conversation,
  }));
}

function glyph(conversation: Conversation): string {
  if (!conversation.canOpen) return "$(circle-slash)";
  if (conversation.open) return "$(check)";
  switch (conversation.activity) {
    case "attention":
      return "$(warning)";
    case "working":
      return "$(loading~spin)";
    case "ready":
      return "$(circle-filled)";
    case "saved":
      return "$(circle-outline)";
  }
}
