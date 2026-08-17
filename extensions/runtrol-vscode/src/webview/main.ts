import "./webview.css";
import {
  coalesceChunks,
  number,
  presentationOf,
  record,
  string,
  textOf,
  type UnknownRecord,
} from "./presentation";
import { conversationEmptyCopy, sendShortcutHint } from "./conversationCopy";
import { toolActivityLine, toolActivityOf } from "./toolActivity";
import { afterFrameOrDelay } from "./renderReady";
import {
  agentLine,
  NO_FACTS,
  NO_USAGE,
  usageLine,
  type ConversationFacts,
  type UsageFacts,
} from "./statusLine";
import { limitTelemetry, usageTelemetry } from "./telemetry";
import { sessionTitle } from "../sessionDisplay";
import type { SessionLine as Session } from "../runtimeTypes";


type FrameEnvelope = {
  generation: number;
  payload: unknown;
};

type Incoming =
  | { type: "reset"; session: Session | null; title: string | null; provider: string | null; generation: number }
  | { type: "session"; session: Session; title: string; provider: string }
  | { type: "frames"; batch: FrameEnvelope[]; gap: boolean }
  | { type: "status"; message: string; kind: "info" | "warning" | "error" }
  | { type: "readyProbe" }
  | { type: "measureStart"; id: string }
  | { type: "measureEnd"; id: string; producedFrames: number; droppedFrames: number };

type VsCodeApi = {
  postMessage(message: unknown): void;
};

type Measurement = {
  id: string;
  baselineIntervals: number[];
  baselineFrameP95Ms: number | null;
  frameIntervals: number[];
  inputLatencies: number[];
  scrollLatencies: number[];
  lastFrameAt: number;
  nextInputAt: number;
  nextScrollAt: number;
  maxPendingFrames: number;
  producedFrames: number | null;
  droppedFrames: number | null;
  completing: boolean;
  ready: boolean;
};

declare function acquireVsCodeApi(): VsCodeApi;

const MAX_VISIBLE_ITEMS = 400;
const MAX_VISIBLE_CHARACTERS = 256 * 1024;
const MAX_MESSAGE_CHARACTERS = 8 * 1024;
const MAX_BATCH = 240;
const MAX_PENDING_FRAMES = 4_096;
const BASELINE_FRAMES = 30;
const SELECTION_RENDER_FALLBACK_MS = 250;
const LOCALIZED_TEXT: Record<string, string> = {
  "session.attached": "Chat opened",
  "session.detached": "Chat saved",
  "session.updated": "Chat information changed",
  "tool.started": "Tool call started",
  "tool.updated": "Tool call updated",
  "plan.updated": "Plan updated",
  "commands.updated": "Available commands changed",
  "mode.updated": "Agent mode changed",
  "configuration.updated": "Configuration changed",
  "approval.waiting": "Approval required",
  "approval.withdrawn": "Approval was withdrawn",
};
const HIDDEN_STATUS_KEYS = new Set([
  "session.attached",
  "session.detached",
  "session.updated",
  "commands.updated",
  "mode.updated",
  "configuration.updated",
]);
const vscode = acquireVsCodeApi();
const status = element<HTMLDivElement>("status");
const conversation = element<HTMLElement>("conversation");
const composer = element<HTMLFormElement>("composer");
const prompt = element<HTMLTextAreaElement>("prompt");
const send = element<HTMLButtonElement>("send");
const sendHint = element<HTMLSpanElement>("send-hint");
const interrupt = element<HTMLButtonElement>("interrupt");
const agentChip = element<HTMLSpanElement>("agent-chip");
const usageChip = element<HTMLSpanElement>("usage-chip");
const pending: unknown[] = [];
let selected: Session | null = null;
let generation = 0;
let pendingHead = 0;
let scheduled = false;
let visibleCharacters = 0;
let measurement: Measurement | null = null;
let followsTail = true;
let currentProvider = "Coding agent";
let promptWasSendable = false;
let currentTitle: string | null = null;
let facts: ConversationFacts = NO_FACTS;
let usage: UsageFacts = NO_USAGE;

window.addEventListener("message", ({ data }: MessageEvent<Incoming>) => {
  if (data.type === "reset") {
    reset(data.session, data.title, data.provider, data.generation);
    return;
  }
  if (data.type === "session") {
    selected = data.session;
    renderSession(data.session, data.title, data.provider);
    return;
  }
  if (data.type === "status") {
    setStatus(data.message, data.kind);
    return;
  }
  if (data.type === "readyProbe") {
    vscode.postMessage({ type: "webviewReady", probe: true });
    return;
  }
  if (data.type === "measureStart") {
    startMeasurement(data.id);
    return;
  }
  if (data.type === "measureEnd") {
    endMeasurement(data.id, data.producedFrames, data.droppedFrames);
    return;
  }
  const frames = data.batch
    .filter((frame) => frame.generation === generation)
    .map((frame) => frame.payload);
  if (data.gap) {
    setStatus("The active view fell behind its bounded presentation queue.", "warning");
  }
  enqueue(frames);
});

sendHint.textContent = sendShortcutHint();
vscode.postMessage({ type: "webviewReady" });

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = prompt.value;
  if (!selected || !text.trim()) {
    return;
  }
  vscode.postMessage({ type: "prompt", text });
  prompt.value = "";
  resizePrompt();
  prompt.focus();
});
prompt.addEventListener("keydown", (event) => {
  // Enter sends and Shift+Enter writes a new line, which is what every chat surface has taught people to expect.
  // A modifier-to-send binding makes the common action the awkward one.
  if (event.key !== "Enter" || event.isComposing || event.shiftKey || event.altKey) return;
  event.preventDefault();
  composer.requestSubmit();
});
prompt.addEventListener("input", resizePrompt);
interrupt.addEventListener("click", () => vscode.postMessage({ type: "interrupt" }));
conversation.addEventListener("scroll", () => {
  followsTail = conversation.scrollHeight - conversation.scrollTop - conversation.clientHeight < 24;
}, { passive: true });

function reset(
  session: Session | null,
  displayTitle: string | null,
  provider: string | null,
  nextGeneration: number,
): void {
  selected = session;
  generation = nextGeneration;
  pending.length = 0;
  pendingHead = 0;
  conversation.replaceChildren();
  visibleCharacters = 0;
  followsTail = true;
  status.textContent = "";
  status.className = "";
  resetSessionTelemetry();
  promptWasSendable = false;
  renderSession(session, displayTitle, provider);
  prompt.value = "";
  resizePrompt();
  renderEmptyState(session);
  afterFrameOrDelay(
    {
      requestFrame: (callback) => requestAnimationFrame(callback),
      setDelay: (callback, milliseconds) => window.setTimeout(callback, milliseconds),
      clearDelay: (handle) => window.clearTimeout(handle),
    },
    SELECTION_RENDER_FALLBACK_MS,
    () => {
      if (generation === nextGeneration) {
        vscode.postMessage({ type: "selectionRendered", generation: nextGeneration });
      }
    },
  );
}

function renderSession(session: Session | null, displayTitle: string | null, provider: string | null): void {
  currentProvider = provider || "Coding agent";
  currentTitle = session ? displayTitle || sessionTitle(session) : null;
  document.body.classList.toggle("no-chat", !session);
  document.body.classList.toggle("opening", session?.lifecycle === "cold");
  document.body.classList.toggle("working", session?.lifecycle === "hotRunning");
  document.body.classList.toggle("throttled", session?.waitingOn === "quota");
  facts = { ...facts, service: session ? currentProvider : "" };
  paintFacts();
  const canSend = session?.lifecycle === "hotIdle";
  const canInterrupt = session?.lifecycle === "hotRunning";
  // A turn that stopped for a person is still a running turn, so the lifecycle alone would say "working" about
  // the one session that is actually waiting on the reader.
  const waitingOnYou = session?.waitingOn === "person";
  document.body.classList.toggle("waiting", waitingOnYou);
  prompt.disabled = !canSend;
  send.disabled = !canSend;
  send.hidden = !canSend;
  sendHint.hidden = !canSend;
  interrupt.disabled = !canInterrupt;
  interrupt.hidden = !canInterrupt;
  prompt.setAttribute("aria-label", session ? `Message ${currentProvider}` : "Message");
  prompt.placeholder = !session
    ? "Message"
    : canSend
      ? `Message ${currentProvider}`
      : waitingOnYou
        ? `${currentProvider} is waiting for you`
        : session.waitingOn === "quota"
          ? `${currentProvider} is waiting on an account limit`
          : canInterrupt
            ? `${currentProvider} is working`
            : session.lifecycle === "failed"
              ? "This conversation needs attention"
              : "Reopening the saved conversation";
  if (canSend && !promptWasSendable && document.hasFocus()) {
    prompt.focus();
  }
  promptWasSendable = canSend;
}

/// The one line under the composer that replaced an entire panel.
function paintFacts(): void {
  const agent = agentLine(facts);
  agentChip.textContent = agent;
  agentChip.hidden = !agent;
  const spent = usageLine(usage, Date.now());
  usageChip.textContent = spent;
  usageChip.hidden = !spent;
  usageChip.classList.toggle("limit-reached", usage.reached);
}

function renderEmptyState(session: Session | null): void {
  const emptyCopy = conversationEmptyCopy(session, currentProvider, currentTitle);
  const empty = document.createElement("section");
  empty.className = `empty-state empty-${emptyCopy.tone}`;
  empty.dataset.placeholder = "true";
  empty.dataset.characters = "0";
  const mark = document.createElement("div");
  mark.className = "empty-mark";
  mark.textContent = "R";
  const heading = document.createElement("h1");
  const detail = document.createElement("p");
  heading.textContent = emptyCopy.heading;
  detail.textContent = emptyCopy.detail;
  empty.append(mark, heading, detail);
  if (!session) {
    const start = document.createElement("button");
    start.type = "button";
    start.className = "empty-primary";
    start.textContent = "New conversation";
    start.addEventListener("click", () => vscode.postMessage({ type: "startChat" }));
    empty.append(start);
  }
  conversation.append(empty);
}

function enqueue(frames: readonly unknown[]): void {
  const renderFrames = coalesceChunks(frames);
  if (renderFrames.length === 0) {
    return;
  }
  const overflow = pendingCount() + renderFrames.length - MAX_PENDING_FRAMES;
  if (overflow > 0) {
    discardPending(overflow);
    setStatus("The active view fell behind its bounded render queue.", "warning");
  }
  pending.push(...renderFrames.slice(Math.max(0, renderFrames.length - MAX_PENDING_FRAMES)));
  if (measurement) {
    measurement.maxPendingFrames = Math.max(measurement.maxPendingFrames, pendingCount());
  }
  schedule();
}

function schedule(): void {
  if (scheduled) {
    return;
  }
  scheduled = true;
  requestAnimationFrame(flush);
}

function flush(): void {
  scheduled = false;
  const shouldFollowTail = followsTail;
  const count = Math.min(MAX_BATCH, pendingCount());
  for (let index = 0; index < count; index += 1) {
    present(takePending());
  }
  if (shouldFollowTail) {
    conversation.scrollTop = Number.MAX_SAFE_INTEGER;
  }
  compactPending();
  if (pendingCount() > 0) {
    schedule();
  } else {
    finishMeasurementWhenDrained();
  }
}

function pendingCount(): number {
  return pending.length - pendingHead;
}

function takePending(): unknown {
  const value = pending[pendingHead];
  pendingHead += 1;
  return value;
}

function discardPending(count: number): void {
  pendingHead = Math.min(pending.length, pendingHead + count);
  compactPending();
}

function compactPending(): void {
  if (pendingHead === pending.length) {
    pending.length = 0;
    pendingHead = 0;
  } else if (pendingHead >= 1_024 && pendingHead * 2 >= pending.length) {
    pending.splice(0, pendingHead);
    pendingHead = 0;
  }
}

function present(payload: unknown): void {
  const envelope = record(payload);
  const body = record(envelope?.body);
  const event = string(body?.event);
  if (!body || !event) {
    appendMessage("meta", "An unreadable event arrived.");
    return;
  }

  const presentation = presentationOf(event);
  if (!presentation) {
    appendMessage("warning", `Unknown event ${event}`);
    return;
  }
  if (presentation.kind === "message") {
    const text = textOf(body.content);
    const messageId = string(body.message_id);
    appendMessage(presentation.side, text, Boolean(body.delta), messageId);
    return;
  }
  if (presentation.kind === "approval") {
    appendApproval(body);
    return;
  }
  if (presentation.kind === "tool") {
    appendTool(body);
    return;
  }
  if (presentation.kind === "turn") {
    const step = string(body.step) || "updated";
    const stop = string(body.stop);
    appendMessage("meta", stop ? `Turn ${step}: ${stop}` : `Turn ${step}`);
    return;
  }
  if (presentation.kind === "notice") {
    const code = string(body.code) || "provider notice";
    appendMessage("warning", code);
    return;
  }
  if (presentation.kind === "usage") {
    updateUsage(body);
    return;
  }
  if (presentation.kind === "rateLimit") {
    updateRateLimits(body);
    return;
  }
  if (presentation.kind === "status") {
    if (presentation.textKey === "session.attached") updateAttachment(body);
    if (presentation.textKey === "mode.updated") updateMode(body);
    const text = LOCALIZED_TEXT[presentation.textKey];
    if (text && !HIDDEN_STATUS_KEYS.has(presentation.textKey)) appendMessage("meta", text);
  }
}

function resetSessionTelemetry(): void {
  facts = { ...NO_FACTS, service: currentProvider };
  usage = NO_USAGE;
  paintFacts();
}

function updateAttachment(body: UnknownRecord): void {
  facts = {
    ...facts,
    model: string(body.model_requested),
    effort: string(body.reasoning_effort_requested),
  };
  paintFacts();
}

function updateMode(body: UnknownRecord): void {
  facts = { ...facts, mode: string(body.mode_id) };
  paintFacts();
}

function updateUsage(body: UnknownRecord): void {
  usage = { ...usage, usage: usageTelemetry(body) };
  paintFacts();
}

function updateRateLimits(body: UnknownRecord): void {
  usage = {
    ...usage,
    primary: limitTelemetry(record(body.primary)) ?? usage.primary,
    secondary: limitTelemetry(record(body.secondary)) ?? usage.secondary,
    reached: body.reached === true,
  };
  paintFacts();
}

function percentWidth(used: number | null, size: number | null): string {
  return used === null || size === null || size <= 0
    ? "0"
    : `${Math.min(100, (used / size) * 100)}%`;
}

function formatCount(value: number): string {
  return new Intl.NumberFormat(undefined, {
    notation: value >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatDuration(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 1_440) return `${Math.round(minutes / 60)} hr`;
  return `${Math.round(minutes / 1_440)} day`;
}

function formatReset(milliseconds: number): string {
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    ...(new Date(milliseconds).toDateString() !== new Date().toDateString()
      ? { month: "short", day: "numeric" }
      : {}),
  }).format(new Date(milliseconds));
}

function appendMessage(side: string, text: string, delta = false, messageId = ""): void {
  if (!text) {
    return;
  }
  clearPlaceholder();
  if (text.length > MAX_MESSAGE_CHARACTERS) {
    for (let offset = 0; offset < text.length; offset += MAX_MESSAGE_CHARACTERS) {
      appendMessage(side, text.slice(offset, offset + MAX_MESSAGE_CHARACTERS), delta, messageId);
    }
    return;
  }
  const last = conversation.lastElementChild as HTMLElement | null;
  const lastCharacters = Number(last?.dataset.characters ?? 0);
  if (
    delta
    && messageId
    && last?.dataset.messageId === messageId
    && last.dataset.side === side
    && lastCharacters + text.length <= MAX_MESSAGE_CHARACTERS
  ) {
    const tail = last.lastChild;
    if (tail instanceof Text) {
      tail.appendData(text);
    } else {
      last.append(document.createTextNode(text));
    }
    last.dataset.characters = String(lastCharacters + text.length);
    visibleCharacters += text.length;
    trim();
    return;
  }
  const item = document.createElement("article");
  item.className = `message ${side}`;
  item.dataset.side = side;
  if (messageId) {
    item.dataset.messageId = messageId;
  }
  item.dataset.characters = String(text.length);
  const author = messageAuthor(side);
  if (author) {
    const label = document.createElement("span");
    label.className = "message-author";
    label.textContent = author;
    item.append(label);
  }
  item.append(document.createTextNode(text));
  visibleCharacters += text.length;
  conversation.append(item);
  trim();
}

function messageAuthor(side: string): string | null {
  if (side === "mine") return "You";
  if (side === "theirs") return currentProvider;
  if (side === "thought") return `${currentProvider} thinking`;
  return null;
}

function serviceInitials(name: string): string {
  const words = name.split(/\s+/).filter(Boolean);
  const initials = words.slice(0, 2).map((word) => word.charAt(0)).join("");
  return initials.toLocaleUpperCase("en-US") || "R";
}

function appendApproval(body: UnknownRecord): void {
  clearPlaceholder();
  const approval = string(body.id);
  const digest = Array.isArray(body.subject_digest)
    ? body.subject_digest.filter((byte): byte is number => typeof byte === "number")
    : [];
  const incomplete = body.subject_incomplete === true;
  const highRisk = string(body.risk) === "high";
  const card = document.createElement("article");
  card.className = `message approval ${highRisk ? "high-risk" : ""}`;
  const heading = document.createElement("strong");
  heading.textContent = incomplete
    ? "Approval blocked: incomplete subject"
    : highRisk
      ? "High risk. Approval required"
      : "Approval required";
  card.append(heading);
  const subject = document.createElement("pre");
  subject.textContent = printable(body.subject);
  card.append(subject);
  const actions = document.createElement("div");
  actions.className = "approval-actions";
  const options = Array.isArray(body.options) ? body.options : [];
  for (const raw of options) {
    const option = record(raw);
    const id = number(option?.id);
    const kind = string(option?.kind);
    if (id === null) {
      continue;
    }
    const button = document.createElement("button");
    button.type = "button";
    // Styled by what the option does, never by where the provider happened to put it. Refusing and granting must
    // not be one misclick apart, and the provider decides the order of its own options.
    button.dataset.kind = kind.startsWith("reject") ? "reject" : "allow";
    button.dataset.scope = kind.endsWith("Always") ? "always" : "once";
    button.textContent = string(option?.label) || kind || `Option ${id}`;
    button.disabled = incomplete && !kind.startsWith("reject");
    button.addEventListener("click", () => {
      vscode.postMessage({ type: "answerApproval", approval, option: id, subjectDigest: digest });
      for (const sibling of actions.querySelectorAll("button")) {
        sibling.disabled = true;
      }
    });
    actions.append(button);
  }
  card.append(actions);
  visibleCharacters += card.textContent?.length ?? 0;
  card.dataset.characters = String(card.textContent?.length ?? 0);
  conversation.append(card);
  trim();
}

/*
 * What the agent is doing to the project, as one line that updates in place.
 *
 * This used to render the fixed sentence "Tool call started" with no tool, no target and no outcome, so an agent
 * that edited five files and ran the tests showed five identical lines. The provider already sends its own
 * classification and status; only the label is read out of the payload, and the raw input, raw output and diffs
 * stay unread because those are the conversation.
 *
 * Updates replace the line for the same call rather than appending, so a long run does not fill the thread.
 */
function appendTool(body: UnknownRecord): void {
  const activity = toolActivityOf(body);
  const line = toolActivityLine(activity);
  const callId = string(body.tool_call_id) || string(body.toolCallId);
  const existing = callId
    ? conversation.querySelector<HTMLElement>(`[data-tool-call="${cssEscape(callId)}"]`)
    : null;
  const item = existing ?? document.createElement("article");
  if (!existing) {
    clearPlaceholder();
    if (callId) item.dataset.toolCall = callId;
    conversation.append(item);
  }
  item.className = `message tool tool-${activity.state}`;
  visibleCharacters -= Number(item.dataset.characters ?? 0);
  item.textContent = line;
  item.dataset.characters = String(line.length);
  visibleCharacters += line.length;
  trim();
}

/// Only what a CSS attribute selector needs, because a provider identifier is untrusted text.
function cssEscape(value: string): string {
  return value.replaceAll(/["\\]/gu, (match) => `\\${match}`);
}

function clearPlaceholder(): void {
  conversation.querySelector<HTMLElement>("[data-placeholder='true']")?.remove();
}

function trim(): void {
  while (
    conversation.childElementCount > MAX_VISIBLE_ITEMS
    || visibleCharacters > MAX_VISIBLE_CHARACTERS
  ) {
    const oldest = conversation.firstElementChild;
    if (!oldest) {
      visibleCharacters = 0;
      return;
    }
    visibleCharacters -= Number((oldest as HTMLElement).dataset.characters ?? 0);
    oldest.remove();
  }
}

function setStatus(message: string, kind: string): void {
  status.textContent = message;
  status.className = message ? kind : "";
}

function startMeasurement(id: string): void {
  const now = performance.now();
  measurement = {
    id,
    baselineIntervals: [],
    baselineFrameP95Ms: null,
    frameIntervals: [],
    inputLatencies: [],
    scrollLatencies: [],
    lastFrameAt: now,
    nextInputAt: now + 100,
    nextScrollAt: now + 100,
    maxPendingFrames: pendingCount(),
    producedFrames: null,
    droppedFrames: null,
    completing: false,
    ready: false,
  };
  requestAnimationFrame((at) => measureFrame(id, at));
}

function measureFrame(id: string, at: number): void {
  const active = measurement;
  if (!active || active.id !== id) {
    return;
  }
  const interval = at - active.lastFrameAt;
  active.lastFrameAt = at;
  if (!active.ready) {
    active.baselineIntervals.push(interval);
    if (active.baselineIntervals.length >= BASELINE_FRAMES) {
      active.baselineFrameP95Ms = percentile(active.baselineIntervals.slice(5), 0.95);
      active.ready = true;
      active.nextInputAt = at + 100;
      active.nextScrollAt = at + 100;
      vscode.postMessage({ type: "measurementReady", id });
    }
    requestAnimationFrame((next) => measureFrame(id, next));
    return;
  }
  active.frameIntervals.push(interval);
  if (at >= active.nextInputAt) {
    active.nextInputAt = at + 100;
    const started = performance.now();
    prompt.value = `${prompt.value.slice(-31)}x`;
    prompt.dispatchEvent(new InputEvent("input", { data: "x", inputType: "insertText" }));
    requestAnimationFrame(() => {
      if (measurement?.id === id) {
        active.inputLatencies.push(performance.now() - started);
      }
    });
  }
  if (at >= active.nextScrollAt) {
    active.nextScrollAt = at + 100;
    const started = performance.now();
    conversation.scrollTop = conversation.scrollTop > 0 ? 0 : Number.MAX_SAFE_INTEGER;
    requestAnimationFrame(() => {
      if (measurement?.id === id) {
        active.scrollLatencies.push(performance.now() - started);
      }
    });
  }
  requestAnimationFrame((next) => measureFrame(id, next));
}

function endMeasurement(id: string, producedFrames: number, droppedFrames: number): void {
  if (!measurement || measurement.id !== id) {
    return;
  }
  measurement.producedFrames = producedFrames;
  measurement.droppedFrames = droppedFrames;
  finishMeasurementWhenDrained();
}

function finishMeasurementWhenDrained(): void {
  const active = measurement;
  if (
    !active
    || active.producedFrames === null
    || active.droppedFrames === null
    || active.completing
    || pendingCount() > 0
  ) {
    return;
  }
  active.completing = true;
  requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(() => {
    if (measurement !== active) {
      return;
    }
    const frameP95Ms = percentile(active.frameIntervals.slice(5), 0.95);
    const baselineFrameP95Ms = active.baselineFrameP95Ms ?? Number.POSITIVE_INFINITY;
    const metrics = {
      baselineFrameP95Ms,
      frameP95Ms,
      frameOverrunP95Ms: Math.max(0, frameP95Ms - baselineFrameP95Ms),
      inputP95Ms: percentile(active.inputLatencies, 0.95),
      scrollP95Ms: percentile(active.scrollLatencies, 0.95),
      maxPendingFrames: active.maxPendingFrames,
      producedFrames: active.producedFrames,
      droppedFrames: active.droppedFrames,
      visibleCharacters,
      visibleItems: conversation.childElementCount,
    };
    measurement = null;
    vscode.postMessage({ type: "performanceMeasurement", id: active.id, metrics });
  })));
}

function percentile(values: readonly number[], at: number): number {
  if (values.length === 0) {
    return Number.POSITIVE_INFINITY;
  }
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * at) - 1] ?? Number.POSITIVE_INFINITY;
}

function printable(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return "The provider supplied a subject that cannot be displayed.";
  }
}

function resizePrompt(): void {
  prompt.style.height = "auto";
  prompt.style.height = `${Math.min(prompt.scrollHeight, 224)}px`;
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) {
    throw new Error(`missing webview element ${id}`);
  }
  return found as T;
}
