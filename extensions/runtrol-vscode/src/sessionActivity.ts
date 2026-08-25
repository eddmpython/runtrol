import { record, string } from "./events/presentation";
import { toolActivityLine, toolActivityOf } from "./events/toolActivity";

/// What a running conversation is doing right now, read from the provider's own events and shown on its
/// sidebar row so the reader knows without opening it.
///
/// Two facts and nothing else: the tool the provider says is running (its own name or its own classification,
/// the same line the conversation page draws for that call) and whether the provider said it needs the
/// operator to sign in. No payload is read beyond the fields the page already reads for the same line; nothing
/// is kept once the turn ends.
export type SessionActivity = {
  readonly tool: string | null;
  readonly signInNeeded: boolean;
};

export const NO_ACTIVITY: SessionActivity = { tool: null, signInNeeded: false };

/// The activity after one event, as the watch delivers it (`{ body: { event, ... } }`).
///
/// A tool call that starts or is still running names the tool; its completion, failure or cancellation, and the
/// end of the turn, clear it. A provider notice coded `authRequired` raises the sign-in flag, which the next
/// attachment (a fresh process that got past its login) lowers.
export function activityAfter(previous: SessionActivity, payload: unknown): SessionActivity {
  const body = record(record(payload)?.body);
  if (!body) return previous;
  const event = string(body.event);
  if (event === "toolCall" || event === "toolCallUpdate") {
    const activity = toolActivityOf(body);
    if (activity.state === "done" || activity.state === "failed" || activity.state === "cancelled") {
      return previous.tool === null ? previous : { ...previous, tool: null };
    }
    // The row wants the word, not the page's "..." running suffix: the same line, read as settled.
    const line = toolActivityLine({ ...activity, state: "done" });
    if (!line || line === previous.tool) return previous;
    // A result frame names no tool (Claude Code's tool_result carries only an identifier); the name already
    // on the row stays, as the page keeps it for the same reason.
    if (event === "toolCallUpdate" && !activity.target && previous.tool) return previous;
    return { ...previous, tool: line };
  }
  if (event === "turn" && string(body.step) === "ended") {
    return previous.tool === null ? previous : { ...previous, tool: null };
  }
  if (event === "notice" && string(body.code) === "authRequired") {
    return previous.signInNeeded ? previous : { ...previous, signInNeeded: true };
  }
  if (event === "attached" && previous.signInNeeded) {
    return { ...previous, signInNeeded: false };
  }
  return previous;
}

export function sameActivity(left: SessionActivity, right: SessionActivity): boolean {
  return left.tool === right.tool && left.signInNeeded === right.signInNeeded;
}
