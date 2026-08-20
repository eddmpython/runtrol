/// Messages typed while the agent was still working, waiting their turn.
///
/// Memory of this document only, on purpose. The extension never stores a draft (that is a written
/// contract), and this queue is the same kind of thing as the composer's own unsent text: it lives
/// exactly as long as the page. Switching sessions or hiding the tab discards it, and the strip that
/// shows the queue is also the honest statement of that lifetime.
///
/// Bounded because everything crossing this page is: eight messages of eight kilobytes covers a
/// person typing ahead, and anything past that is a script, not a queue.

const MAX_QUEUED_MESSAGES = 8;
export const MAX_QUEUED_MESSAGE_CHARACTERS = 8 * 1024;

export type QueueOutcome =
  | { accepted: true; queue: readonly string[] }
  | { accepted: false; queue: readonly string[]; why: string };

export function pushQueued(queue: readonly string[], text: string): QueueOutcome {
  const trimmed = text.trim();
  if (!trimmed) {
    return { accepted: false, queue, why: "an empty message has nothing to queue" };
  }
  if (queue.length >= MAX_QUEUED_MESSAGES) {
    return {
      accepted: false,
      queue,
      why: `the queue holds ${MAX_QUEUED_MESSAGES} messages; send or cancel one first`,
    };
  }
  if (text.length > MAX_QUEUED_MESSAGE_CHARACTERS) {
    return { accepted: false, queue, why: "the message is too long to queue" };
  }
  return { accepted: true, queue: [...queue, text] };
}

export function cancelQueued(queue: readonly string[], index: number): readonly string[] {
  if (!Number.isInteger(index) || index < 0 || index >= queue.length) return queue;
  return [...queue.slice(0, index), ...queue.slice(index + 1)];
}

/// The one message to send now, first in first out.
export function takeQueued(queue: readonly string[]): { next: string | null; queue: readonly string[] } {
  const [next, ...rest] = queue;
  return next === undefined ? { next: null, queue } : { next, queue: rest };
}

/// How one queued message reads on the strip.
export function queuedLabel(text: string): string {
  const oneLine = text.replaceAll(/\s+/gu, " ").trim();
  return oneLine.length > 80 ? `${oneLine.slice(0, 79)}…` : oneLine;
}
