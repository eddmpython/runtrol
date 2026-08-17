export type ConversationEmptyCopy = {
  heading: string;
  detail: string;
  tone: "hero" | "quiet";
};

export function conversationEmptyCopy(
  session: { lifecycle: "hotIdle" | "hotRunning" | "failed" | "cold" } | null,
  provider: string,
  title: string | null,
): ConversationEmptyCopy {
  if (!session) {
    return {
      heading: "Start a conversation",
      detail: "Every conversation on this machine is listed in the sidebar.",
      tone: "hero",
    };
  }
  if (session.lifecycle === "hotIdle") {
    return {
      heading: title || "Ready",
      detail: `Message ${provider} below.`,
      tone: "quiet",
    };
  }
  if (session.lifecycle === "hotRunning") {
    return {
      heading: `${provider} is working`,
      detail: "The reply will appear here.",
      tone: "quiet",
    };
  }
  if (session.lifecycle === "failed") {
    return {
      heading: "This conversation needs attention",
      detail: "Read the notice above, then reopen it from the sidebar.",
      tone: "quiet",
    };
  }
  return {
    heading: title || "Reopening",
    detail: "Reopening the saved conversation.",
    tone: "quiet",
  };
}

/// The hint under the composer.
///
/// Enter sends. The only thing worth telling someone is how to write a second line, because that is the key they
/// would otherwise press by accident.
export function sendShortcutHint(): string {
  return "Shift+Enter for a new line";
}
