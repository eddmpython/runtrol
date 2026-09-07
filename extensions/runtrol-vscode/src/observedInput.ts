import type { Conversation } from "./conversationList";

/** Availability is a Runtime authority fact; the sidebar never infers it from a window or process id. */
export function canOpenInputView(row: Pick<Conversation, "presence" | "canOpen" | "hostedTerminal">): boolean {
  return row.presence.kind === "hosted"
    && row.canOpen
    && row.hostedTerminal?.origin === "observedMirror"
    && row.hostedTerminal.processState === "running"
    && row.hostedTerminal.ownerInputAvailable === true;
}
