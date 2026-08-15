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
import { afterFrameOrDelay } from "./renderReady";
import { sessionTitle } from "../sessionDisplay";
import { sessionStateLabel } from "../runtimeProjection";
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
  "session.attached": "Session attached",
  "session.detached": "Session detached",
  "session.updated": "Session information changed",
  "tool.started": "Tool call started",
  "tool.updated": "Tool call updated",
  "plan.updated": "Plan updated",
  "commands.updated": "Available commands changed",
  "mode.updated": "Agent mode changed",
  "configuration.updated": "Configuration changed",
  "approval.waiting": "Approval required",
  "approval.withdrawn": "Approval was withdrawn",
};
const vscode = acquireVsCodeApi();
const title = element<HTMLElement>("session-title");
const sessionPath = element<HTMLElement>("session-path");
const serviceName = element<HTMLElement>("service-name");
const serviceAvatar = element<HTMLElement>("service-avatar");
const composerService = element<HTMLElement>("composer-service");
const sessionState = element<HTMLSpanElement>("session-state");
const status = element<HTMLDivElement>("status");
const conversation = element<HTMLElement>("conversation");
const composer = element<HTMLFormElement>("composer");
const prompt = element<HTMLTextAreaElement>("prompt");
const send = element<HTMLButtonElement>("send");
const interrupt = element<HTMLButtonElement>("interrupt");
const close = element<HTMLButtonElement>("close");
const openWorkspace = element<HTMLButtonElement>("open-workspace");
const pending: unknown[] = [];
let selected: Session | null = null;
let generation = 0;
let pendingHead = 0;
let scheduled = false;
let visibleCharacters = 0;
let measurement: Measurement | null = null;
let followsTail = true;
let currentProvider = "Coding agent";

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
  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    composer.requestSubmit();
  }
});
prompt.addEventListener("input", resizePrompt);
interrupt.addEventListener("click", () => vscode.postMessage({ type: "interrupt" }));
close.addEventListener("click", () => vscode.postMessage({ type: "close" }));
openWorkspace.addEventListener("click", () => vscode.postMessage({ type: "openWorkspace" }));
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
  renderSession(session, displayTitle, provider);
  prompt.value = "";
  resizePrompt();
  if (session) {
    appendMessage("meta", `Connected to ${currentProvider} · ${sessionStateLabel(session)}`);
  } else {
    renderEmptyState();
  }
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
  title.textContent = session ? displayTitle || sessionTitle(session) : "No active chat";
  sessionPath.textContent = session?.workspace ?? "Choose a service from Chats";
  serviceName.textContent = session ? currentProvider : "Select a service";
  serviceAvatar.textContent = session ? serviceInitials(currentProvider) : "R";
  composerService.textContent = session ? currentProvider : "No service selected";
  sessionState.textContent = session ? sessionStateLabel(session) : "";
  sessionState.className = session ? (session.hot ? "hot" : "cold") : "";
  const interactive = session?.hot === true;
  prompt.disabled = !interactive;
  send.disabled = !interactive;
  interrupt.disabled = !interactive;
  close.disabled = !session;
  openWorkspace.disabled = !session;
  openWorkspace.hidden = !session;
  prompt.setAttribute("aria-label", session ? `Message ${currentProvider}` : "Message");
  prompt.placeholder = !session
    ? "Select a service and chat"
    : interactive
      ? `Message ${currentProvider}`
      : "Resuming the provider-owned session";
}

function renderEmptyState(): void {
  const empty = document.createElement("section");
  empty.className = "empty-state";
  empty.dataset.characters = "0";
  const mark = document.createElement("div");
  mark.className = "empty-mark";
  mark.textContent = "R";
  const heading = document.createElement("h1");
  heading.textContent = "Choose a service to start chatting";
  const detail = document.createElement("p");
  detail.textContent = "Open Chats in the Runtrol sidebar, then select a service or one of its existing chats.";
  empty.append(mark, heading, detail);
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
  if (presentation.kind === "status") {
    appendMessage("meta", LOCALIZED_TEXT[presentation.textKey] ?? presentation.textKey);
  }
}

function appendMessage(side: string, text: string, delta = false, messageId = ""): void {
  if (!text) {
    return;
  }
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
  const approval = string(body.id);
  const digest = Array.isArray(body.subject_digest)
    ? body.subject_digest.filter((byte): byte is number => typeof byte === "number")
    : [];
  const incomplete = body.subject_incomplete === true;
  const card = document.createElement("article");
  card.className = `message approval ${string(body.risk) === "high" ? "high-risk" : ""}`;
  const heading = document.createElement("strong");
  heading.textContent = incomplete ? "Approval blocked: incomplete subject" : "Approval required";
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
