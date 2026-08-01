import type { ConversationItem } from "./domain";

const MAX_VISIBLE_ITEMS = 400;
const MAX_VISIBLE_CHARACTERS = 256 * 1024;
const MAX_STREAM_CHUNK_CHARACTERS = 1024;

type UnknownRecord = Record<string, unknown>;

const PRESENTATION: Record<string, Pick<ConversationItem, "side" | "label">> = {
  userMessageChunk: { side: "mine", label: "나" },
  agentMessageChunk: { side: "theirs", label: "에이전트" },
  agentThoughtChunk: { side: "thought", label: "생각" },
};

let nextKey = 0;

function record(value: unknown): UnknownRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as UnknownRecord)
    : null;
}

function string(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function textFromParts(value: unknown): string {
  if (!Array.isArray(value)) {
    return "";
  }
  return value
    .map((part) => string(record(part)?.text))
    .join("");
}

function textOf(payload: unknown): string {
  if (typeof payload === "string") {
    return payload;
  }
  const object = record(payload);
  if (!object) {
    return "";
  }
  const direct = string(object.delta) || string(object.text);
  if (direct) {
    return direct;
  }
  const item = record(object.item);
  if (item && typeof item.text === "string") {
    return item.text;
  }
  const message = record(object.message);
  return textFromParts(message?.content) || textFromParts(object.content);
}

function item(
  side: ConversationItem["side"],
  label: string,
  text: string,
  messageId: string | null = null,
): ConversationItem {
  nextKey += 1;
  return { key: nextKey, side, label, text, messageId };
}

function turnText(body: UnknownRecord): string {
  const step = string(body.step);
  if (step === "accepted") {
    return "접수됨";
  }
  if (step === "started") {
    return "시작됨";
  }
  if (step !== "ended") {
    return step || "턴 상태가 바뀌었다";
  }
  const declaredBy = record(body.declared_by);
  const qualifier = declaredBy?.by === "provider" ? "" : " (공급자가 말한 것이 아님)";
  return `턴 끝 · ${string(body.stop) || "상태 없음"}${qualifier}`;
}

export function frameToItem(frame: string): { item: ConversationItem; isDelta: boolean } {
  let parsed: UnknownRecord | null = null;
  try {
    parsed = record(JSON.parse(frame));
  } catch (error) {
    // An unreadable provider frame stays visible as an explicit event instead of disappearing.
    console.warn("cannot read a provider frame", error);
  }
  const body = record(parsed?.body);
  if (!body) {
    return { item: item("meta", "", "읽을 수 없는 프레임이 왔다"), isDelta: false };
  }

  const event = string(body.event) || "알 수 없는 이벤트";
  if (event === "turn") {
    return { item: item("meta", "", turnText(body)), isDelta: false };
  }
  if (event === "notice") {
    return {
      item: item("meta", "", `알림 · ${string(body.code) || "내용 없음"}`),
      isDelta: false,
    };
  }

  const known = PRESENTATION[event];
  const text = textOf(body.content ?? body.payload);
  if (!known) {
    return {
      item: item("meta", "", `${event}${text ? ` · ${text}` : ""}`),
      isDelta: false,
    };
  }
  const messageId = body.message_id === undefined ? null : String(body.message_id);
  return {
    item: item(known.side, known.label, text, messageId),
    isDelta: Boolean(body.delta),
  };
}

function characterCount(items: readonly ConversationItem[]): number {
  return items.reduce((total, current) => total + current.text.length, 0);
}

function bounded(items: ConversationItem[]): ConversationItem[] {
  let result = items.slice(-MAX_VISIBLE_ITEMS);
  while (result.length > 1 && characterCount(result) > MAX_VISIBLE_CHARACTERS) {
    result = result.slice(1);
  }
  if (result.length === 1 && result[0].text.length > MAX_VISIBLE_CHARACTERS) {
    result = [{ ...result[0], text: result[0].text.slice(-MAX_VISIBLE_CHARACTERS) }];
  }
  return result;
}

export function appendFrame(
  current: readonly ConversationItem[],
  next: ConversationItem,
  isDelta: boolean,
): ConversationItem[] {
  const last = current.at(-1);
  if (
    isDelta
    && next.messageId
    && last?.messageId === next.messageId
    && last.text.length + next.text.length <= MAX_STREAM_CHUNK_CHARACTERS
  ) {
    return bounded([
      ...current.slice(0, -1),
      { ...last, text: `${last.text}${next.text}` },
    ]);
  }
  return bounded([...current, next]);
}

export type PendingFrame = {
  item: ConversationItem;
  isDelta: boolean;
};

export function appendFrames(
  current: readonly ConversationItem[],
  pending: readonly PendingFrame[],
): ConversationItem[] {
  if (pending.length === 0) {
    return [...current];
  }

  const next = [...current];
  for (const frame of pending) {
    const last = next.at(-1);
    if (
      frame.isDelta
      && frame.item.messageId
      && last?.messageId === frame.item.messageId
      && last.text.length + frame.item.text.length <= MAX_STREAM_CHUNK_CHARACTERS
    ) {
      next[next.length - 1] = { ...last, text: `${last.text}${frame.item.text}` };
    } else {
      next.push(frame.item);
    }
  }
  return bounded(next);
}

export function appendStatus(current: readonly ConversationItem[], text: string): ConversationItem[] {
  return bounded([...current, item("meta", "", text)]);
}

export class ConversationFeed {
  private current: readonly ConversationItem[] = [];
  private readonly listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly snapshot = (): readonly ConversationItem[] => this.current;

  clear(): void {
    this.replace([]);
  }

  append(frames: readonly PendingFrame[]): void {
    this.replace(appendFrames(this.current, frames));
  }

  status(text: string): void {
    this.replace(appendStatus(this.current, text));
  }

  private replace(next: readonly ConversationItem[]): void {
    this.current = next;
    for (const listener of this.listeners) {
      listener();
    }
  }
}
