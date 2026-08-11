import "./webview.css";

type UnknownRecord = Record<string, unknown>;

type Session = {
  session: string;
  provider: string;
  workspace: string;
  doing: string;
};

type Incoming =
  | { type: "reset"; session: Session | null }
  | { type: "frame"; payload: unknown }
  | { type: "status"; message: string; kind: "info" | "warning" | "error" };

type VsCodeApi = {
  postMessage(message: unknown): void;
};

declare function acquireVsCodeApi(): VsCodeApi;

const MAX_VISIBLE_ITEMS = 400;
const MAX_VISIBLE_CHARACTERS = 256 * 1024;
const MAX_BATCH = 64;
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
let scheduled = false;
let visibleCharacters = 0;

window.addEventListener("message", ({ data }: MessageEvent<Incoming>) => {
  if (data.type === "reset") {
    reset(data.session);
    return;
  }
  if (data.type === "status") {
    setStatus(data.message, data.kind);
    return;
  }
  pending.push(data.payload);
  schedule();
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

function reset(session: Session | null): void {
  selected = session;
  pending.length = 0;
  conversation.replaceChildren();
  visibleCharacters = 0;
  status.textContent = "";
  status.className = "";
  title.textContent = session ? `${folderName(session.workspace)}  ${session.provider}` : "No active session";
  sessionPath.textContent = session?.workspace ?? "Select a session from the list.";
  prompt.disabled = !session;
  send.disabled = !session;
  interrupt.disabled = !session;
  close.disabled = !session;
  openWorkspace.disabled = !session;
  prompt.placeholder = session
    ? "Send unchanged text to the provider CLI"
    : "Select a session to send a prompt";
  if (session) {
    appendMessage("meta", `Connected to ${session.doing}`);
  }
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
  const count = Math.min(MAX_BATCH, pending.length);
  for (let index = 0; index < count; index += 1) {
    present(pending.shift());
  }
  if (pending.length > 0) {
    schedule();
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

  if (event === "userMessageChunk" || event === "agentMessageChunk" || event === "agentThoughtChunk") {
    const side = event === "userMessageChunk" ? "mine" : event === "agentThoughtChunk" ? "thought" : "theirs";
    const text = textOf(body.content);
    const messageId = string(body.message_id);
    appendMessage(side, text, Boolean(body.delta), messageId);
    return;
  }
  if (event === "approvalRequested") {
    appendApproval(body);
    return;
  }
  if (event === "turn") {
    const step = string(body.step) || "updated";
    const stop = string(body.stop);
    appendMessage("meta", stop ? `Turn ${step}: ${stop}` : `Turn ${step}`);
    return;
  }
  if (event === "notice") {
    const code = string(body.code) || "provider notice";
    appendMessage("warning", code);
    return;
  }
  const statusText: Record<string, string> = {
    attached: "Session attached",
    detached: "Session detached",
    toolCall: "Tool call started",
    toolCallUpdate: "Tool call updated",
    plan: "Plan updated",
    availableCommandsUpdate: "Available commands changed",
    currentModeUpdate: "Agent mode changed",
    configOptionUpdate: "Configuration changed",
    sessionInfoUpdate: "Session information changed",
    approvalWithdrawn: "Approval was withdrawn",
  };
  const text = statusText[event];
  if (text) {
    appendMessage("meta", text);
  }
}

function appendMessage(side: string, text: string, delta = false, messageId = ""): void {
  if (!text) {
    return;
  }
  const last = conversation.lastElementChild as HTMLElement | null;
  if (delta && messageId && last?.dataset.messageId === messageId && last.dataset.side === side) {
    const previous = last.textContent ?? "";
    last.textContent = `${previous}${text}`;
    visibleCharacters += text.length;
    trim();
    conversation.scrollTop = conversation.scrollHeight;
    return;
  }
  const item = document.createElement("article");
  item.className = `message ${side}`;
  item.dataset.side = side;
  if (messageId) {
    item.dataset.messageId = messageId;
  }
  item.textContent = text;
  visibleCharacters += text.length;
  conversation.append(item);
  trim();
  conversation.scrollTop = conversation.scrollHeight;
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
  conversation.append(card);
  trim();
  conversation.scrollTop = conversation.scrollHeight;
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
    visibleCharacters -= oldest.textContent?.length ?? 0;
    oldest.remove();
  }
}

function setStatus(message: string, kind: string): void {
  status.textContent = message;
  status.className = message ? kind : "";
}

function textOf(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  const source = record(value);
  if (!source) {
    return "";
  }
  const direct = string(source.delta) || string(source.text);
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
  return content.map((part) => string(record(part)?.text)).join("");
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

function record(value: unknown): UnknownRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

function string(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function number(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) ? value : null;
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
