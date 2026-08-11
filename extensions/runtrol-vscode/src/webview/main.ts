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

type Session = {
  session: string;
  provider: string;
  workspace: string;
  hot: boolean;
  doing: string;
};

type FrameEnvelope = {
  generation: number;
  payload: unknown;
};

type Incoming =
  | { type: "reset"; session: Session | null; generation: number }
  | { type: "frames"; batch: FrameEnvelope[]; gap: boolean }
  | { type: "status"; message: string; kind: "info" | "warning" | "error" }
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
const sessionPath = element<HTMLDivElement>("session-path");
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

window.addEventListener("message", ({ data }: MessageEvent<Incoming>) => {
  if (data.type === "reset") {
    reset(data.session, data.generation);
    return;
  }
  if (data.type === "status") {
    setStatus(data.message, data.kind);
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

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = prompt.value;
  if (!selected || !text.trim()) {
    return;
  }
  vscode.postMessage({ type: "prompt", text });
  prompt.value = "";
  prompt.focus();
});
interrupt.addEventListener("click", () => vscode.postMessage({ type: "interrupt" }));
close.addEventListener("click", () => vscode.postMessage({ type: "close" }));
openWorkspace.addEventListener("click", () => vscode.postMessage({ type: "openWorkspace" }));
conversation.addEventListener("scroll", () => {
  followsTail = conversation.scrollHeight - conversation.scrollTop - conversation.clientHeight < 24;
}, { passive: true });

function reset(session: Session | null, nextGeneration: number): void {
  selected = session;
  generation = nextGeneration;
  pending.length = 0;
  pendingHead = 0;
  conversation.replaceChildren();
  visibleCharacters = 0;
  followsTail = true;
  status.textContent = "";
  status.className = "";
  title.textContent = session ? `${folderName(session.workspace)}  ${session.provider}` : "No active session";
  sessionPath.textContent = session?.workspace ?? "Select a session from the list.";
  const interactive = session?.hot === true;
  prompt.disabled = !interactive;
  send.disabled = !interactive;
  interrupt.disabled = !interactive;
  close.disabled = !session;
  openWorkspace.disabled = !session;
  prompt.placeholder = !session
    ? "Select a session to send a prompt"
    : interactive
      ? "Send unchanged text to the provider CLI"
      : "Resuming the provider-owned session";
  if (session) {
    appendMessage("meta", `Connected to ${session.doing}`);
  }
  requestAnimationFrame(() => {
    vscode.postMessage({ type: "selectionRendered", generation: nextGeneration });
  });
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
  item.textContent = text;
  visibleCharacters += text.length;
  conversation.append(item);
  trim();
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

function folderName(workspace: string): string {
  const parts = workspace.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.at(-1) ?? workspace;
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) {
    throw new Error(`missing webview element ${id}`);
  }
  return found as T;
}
