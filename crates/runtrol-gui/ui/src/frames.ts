import type { ConversationItem, LimitWindow, RateLimitGauge, UsageGauge } from "./domain";
import eventPresentation from "../../../../assets/event-presentation.json";

const MAX_VISIBLE_ITEMS = 400;
const MAX_VISIBLE_CHARACTERS = 256 * 1024;
const MAX_STREAM_CHUNK_CHARACTERS = 1024;

type UnknownRecord = Record<string, unknown>;

type PresentationContract =
  | { kind: "message"; side: ConversationItem["side"]; labelKey: string }
  | { kind: "status" | "approval"; textKey: string }
  | { kind: "turn" | "notice" | "usage" | "rateLimit" | "discard" };

const EVENT_PRESENTATION = eventPresentation.events as Record<string, PresentationContract>;

const LOCALIZED_TEXT: Record<string, string> = {
  "message.you": "나",
  "message.agent": "에이전트",
  "message.thought": "생각",
  "session.attached": "세션에 연결됐다",
  "session.detached": "세션에서 떨어졌다",
  "session.updated": "세션 정보가 바뀌었다",
  "tool.started": "도구를 호출한다",
  "tool.updated": "도구 호출이 진행 중이다",
  "plan.updated": "계획을 세웠다",
  "commands.updated": "쓸 수 있는 명령이 바뀌었다",
  "mode.updated": "동작 모드가 바뀌었다",
  "configuration.updated": "설정 항목이 바뀌었다",
  "approval.waiting": "승인을 기다린다",
  "approval.withdrawn": "승인 요청을 거뒀다",
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

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function limitWindow(value: unknown): LimitWindow | null {
  const source = record(value);
  const usedPercent = finiteNumber(source?.used_percent);
  if (!source || usedPercent === null) {
    return null;
  }
  return {
    usedPercent,
    resetsAt: finiteNumber(source.resets_at),
    windowMinutes: finiteNumber(source.window_minutes),
  };
}

function usageGauge(body: UnknownRecord): UsageGauge {
  const rawCost = record(body.cost);
  const amount = finiteNumber(rawCost?.amount);
  const currency = string(rawCost?.currency);
  return {
    used: finiteNumber(body.used),
    size: finiteNumber(body.size),
    cost: amount === null || !currency ? null : { amount, currency },
  };
}

function rateLimitGauge(body: UnknownRecord): RateLimitGauge {
  return {
    primary: limitWindow(body.primary),
    secondary: limitWindow(body.secondary),
    reached: body.reached === true,
  };
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

export type PendingFrame = {
  item: ConversationItem | null;
  isDelta: boolean;
  usage?: UsageGauge;
  rateLimit?: RateLimitGauge;
  /** A frame the provider sent and runtrol deliberately does not interpret. Counted, never rendered. */
  unread?: true;
};

export function frameToItem(frame: string): PendingFrame {
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
  const presentation = EVENT_PRESENTATION[event];
  if (!presentation) {
    const text = textOf(body.content ?? body.payload);
    return {
      item: item("meta", "", `알 수 없는 이벤트 ${event}${text ? ` · ${text}` : ""}`),
      isDelta: false,
    };
  }
  if (presentation.kind === "turn") {
    return { item: item("meta", "", turnText(body)), isDelta: false };
  }
  if (presentation.kind === "notice") {
    return {
      item: item("meta", "", `알림 · ${string(body.code) || "내용 없음"}`),
      isDelta: false,
    };
  }
  if (presentation.kind === "usage") {
    return { item: null, isDelta: false, usage: usageGauge(body) };
  }
  if (presentation.kind === "rateLimit") {
    return { item: null, isDelta: false, rateLimit: rateLimitGauge(body) };
  }

  // A frame runtrol relays without reading is not conversation, and drawing one chat line per frame put
  // twelve lines reading `unmapped` in front of an operator who had not sent a turn yet. It is counted
  // and reported beside the other diagnostics instead: nothing is hidden, and the conversation pane is
  // for the conversation. Thinness is about not interpreting a payload, not about rendering every frame.
  if (presentation.kind === "discard") {
    return { item: null, isDelta: false, unread: true };
  }

  const text = textOf(body.content ?? body.payload);
  if (presentation.kind === "status" || presentation.kind === "approval") {
    return { item: item("meta", "", LOCALIZED_TEXT[presentation.textKey] ?? presentation.textKey), isDelta: false };
  }
  if (presentation.kind !== "message") {
    return { item: item("meta", "", "알 수 없는 표현 계약"), isDelta: false };
  }
  const messageId = body.message_id === undefined ? null : String(body.message_id);
  return {
    item: item(
      presentation.side,
      LOCALIZED_TEXT[presentation.labelKey] ?? presentation.labelKey,
      text,
      messageId,
    ),
    isDelta: Boolean(body.delta),
  };
}

function characterCount(items: readonly ConversationItem[]): number {
  return items.reduce((total, current) => total + current.text.length, 0);
}

function bounded(items: ConversationItem[]): ConversationItem[] {
  let result = items.slice(-MAX_VISIBLE_ITEMS);
  let characters = characterCount(result);
  while (result.length > 0 && characters > MAX_VISIBLE_CHARACTERS) {
    const excess = characters - MAX_VISIBLE_CHARACTERS;
    const oldest = result[0];
    if (oldest.text.length <= excess) {
      characters -= oldest.text.length;
      result = result.slice(1);
      continue;
    }
    result = [
      { ...oldest, text: oldest.text.slice(excess) },
      ...result.slice(1),
    ];
    characters -= excess;
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

export function appendFrames(
  current: readonly ConversationItem[],
  pending: readonly PendingFrame[],
): ConversationItem[] {
  if (pending.length === 0) {
    return [...current];
  }

  const next = [...current];
  for (const frame of pending) {
    if (!frame.item) {
      continue;
    }
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
