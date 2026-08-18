import eventPresentation from "../../../../assets/event-presentation.json";

export type UnknownRecord = Record<string, unknown>;

export type PresentationContract =
  | { kind: "message"; side: "mine" | "theirs" | "thought"; labelKey: string }
  | { kind: "status" | "approval"; textKey: string }
  | { kind: "tool"; part: "call" | "result" }
  | { kind: "turn" | "notice" | "usage" | "rateLimit" | "discard" };

const EVENT_PRESENTATION = eventPresentation.events as Record<string, PresentationContract>;

export function presentationOf(event: string): PresentationContract | null {
  return EVENT_PRESENTATION[event] ?? null;
}

export function coalesceChunks(frames: readonly unknown[]): unknown[] {
  const result: unknown[] = [];
  for (const frame of frames) {
    const lastIndex = result.length - 1;
    const merged = lastIndex >= 0 ? mergeChunkPair(result[lastIndex], frame) : null;
    if (merged) {
      result[lastIndex] = merged;
    } else {
      result.push(frame);
    }
  }
  return result;
}

export function textOf(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  const source = record(value);
  if (!source) {
    return "";
  }
  // A streaming delta arrives as an object, not a string. Claude Code sends the content-block shape
  // (`{delta: {type: "text_delta", text}}`, or `thinking` for a thought), and reading `delta` as a string returned
  // nothing for every one of them. The effect was that finished messages appeared and the typing did not: a reply
  // landed in one lump after the agent had already stopped.
  const delta = record(source.delta);
  const direct = string(source.delta)
    || string(delta?.text)
    || string(delta?.thinking)
    || string(source.text)
    || string(source.thinking);
  if (direct) {
    return direct;
  }
  const item = record(source.item);
  if (item && typeof item.text === "string") {
    return item.text;
  }
  const content = Array.isArray(source.content)
    ? source.content
    : Array.isArray(record(source.message)?.content)
      ? record(source.message)?.content as unknown[]
      : [];
  // A content block is text or a thought; both are the service's own words and both belong on screen.
  return content
    .map((part) => string(record(part)?.text) || string(record(part)?.thinking))
    .join("");
}

export function record(value: unknown): UnknownRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

export function string(value: unknown): string {
  return typeof value === "string" ? value : "";
}

export function number(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) ? value : null;
}

function mergeChunkPair(left: unknown, right: unknown): unknown | null {
  const leftEnvelope = record(left);
  const rightEnvelope = record(right);
  const leftBody = record(leftEnvelope?.body);
  const rightBody = record(rightEnvelope?.body);
  const event = string(leftBody?.event);
  const presentation = presentationOf(event);
  if (
    !leftEnvelope
    || !rightEnvelope
    || !leftBody
    || !rightBody
    || presentation?.kind !== "message"
    || event !== string(rightBody.event)
    || !rightBody.delta
    || !string(leftBody.message_id)
    || string(leftBody.message_id) !== string(rightBody.message_id)
  ) {
    return null;
  }
  const leftText = textOf(leftBody.content);
  const rightText = textOf(rightBody.content);
  if (!leftText || !rightText) {
    return null;
  }
  return {
    ...leftEnvelope,
    body: {
      ...leftBody,
      content: { text: `${leftText}${rightText}` },
    },
  };
}
