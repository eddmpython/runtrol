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
import { hasMarkdownTrigger, parseMarkdown, type Block, type Inline } from "./markdown";
import { insertMention, mentionTriggered } from "./mention";
import { planEntriesOf, planGlyph } from "./plan";
import { cancelQueued, pushQueued, queuedLabel, takeQueued } from "./queue";
import { toolActivityLineKeeping, toolActivityOf } from "./toolActivity";
import { toolDetail } from "./toolDetail";
import { declaredDiffs, type DeclaredDiff } from "./toolDiff";
import {
  asksForCommands,
  completed,
  matchingCommands,
  movedHighlight,
  slashCommandsOf,
  type SlashCommand,
} from "./slashCommands";
import { afterFrameOrDelay } from "./renderReady";
import {
  chipText,
  modelLine,
  NO_FACTS,
  NO_USAGE,
  usageLine,
  type ConversationFacts,
  type UsageFacts,
} from "./statusLine";
import { limitTelemetry, usageTelemetry } from "./telemetry";
import { draftGreeting, type DraftChips } from "../draft";
import { sessionTitle } from "../sessionDisplay";
import type { SessionLine as Session } from "../runtimeTypes";

/// Where a live conversation runs, as the host says it.
type ConversationContext = {
  project: string;
  projectPath: string | null;
  branch: string | null;
};

/// One attachment as the host lists it.
type AttachmentLabel = { name: string; kilobytes: number };


type FrameEnvelope = {
  generation: number;
  payload: unknown;
};

type Incoming =
  | {
    type: "reset";
    session: Session | null;
    title: string | null;
    provider: string | null;
    generation: number;
    draft: DraftChips | null;
    draftState: unknown;
  }
  | { type: "session"; session: Session; title: string; provider: string }
  | { type: "draft"; draft: DraftChips; draftState: unknown }
  | { type: "context"; context: ConversationContext }
  | { type: "attachments"; items: AttachmentLabel[] }
  | { type: "frames"; batch: FrameEnvelope[]; gap: boolean }
  | { type: "status"; message: string; kind: "info" | "warning" | "error" }
  | { type: "readyProbe" }
  | { type: "measureStart"; id: string }
  | { type: "measureEnd"; id: string; producedFrames: number; droppedFrames: number }
  | { type: "switchRequested"; what: "model" | "mode" | "effort"; value: string }
  | { type: "insertText"; text: string | null }
  | { type: "openLatestDiff" }
  | { type: "menu"; menu: string; anchor: MenuAnchor; title: string; items: MenuItem[] }
  | { type: "menuClose"; menu: string }
  | { type: "clickChip"; anchor: MenuAnchor };

type MenuAnchor = "project" | "service" | "model" | "effort" | "mode";
type MenuItem = { id: string; label: string; description?: string; detail?: string };

type VsCodeApi = {
  setState(state: unknown): void;
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
  "model.updated": "Answering model changed",
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
  "model.updated",
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
// Every chip is also its switch: the place that says a fact is the place to change it. On a draft the chips
// choose what the conversation will start with; on a live conversation they switch the running session
// (the project and service chips then offer the honest next thing: a new conversation here).
const projectChip = element<HTMLButtonElement>("project-chip");
projectChip.addEventListener("click", () => vscode.postMessage({ type: "pickProject" }));
const branchChip = element<HTMLSpanElement>("branch-chip");
const serviceChip = element<HTMLButtonElement>("service-chip");
serviceChip.addEventListener("click", () => vscode.postMessage({ type: "pickService" }));
const modelChip = element<HTMLButtonElement>("model-chip");
modelChip.addEventListener("click", () => {
  vscode.postMessage({ type: "switchModel", available: switchableModels });
});
const modeChip = element<HTMLButtonElement>("mode-chip");
modeChip.addEventListener("click", () => {
  vscode.postMessage({ type: "switchMode", available: switchableModeIds });
});
// Its own chip because the model is a confirmed value and the effort a requested one, and one chip must
// not mix the two kinds of fact.
const effortChip = element<HTMLButtonElement>("effort-chip");
effortChip.addEventListener("click", () => {
  vscode.postMessage({ type: "switchEffort", model: facts.model });
});
const attach = element<HTMLButtonElement>("attach");
attach.addEventListener("click", () => vscode.postMessage({ type: "attach" }));
const attachmentList = element<HTMLUListElement>("attachments");
const usageChip = element<HTMLSpanElement>("usage-chip");
const commandMenu = element<HTMLUListElement>("commands");
const chipMenu = element<HTMLUListElement>("chip-menu");
/// The popover a chip opened, if one is open: which question, its items, and the highlighted row.
let openMenu: { id: string; anchor: MenuAnchor; items: MenuItem[]; highlighted: number } | null = null;
const queuedList = element<HTMLUListElement>("queued");
const pending: unknown[] = [];
let selected: Session | null = null;
/// The draft this page shows while no session exists: the chips of a conversation that has not started.
let draft: DraftChips | null = null;
/// Where the live conversation runs, once the host has said.
let context: ConversationContext | null = null;
let generation = 0;
let pendingHead = 0;
let scheduled = false;
let visibleCharacters = 0;
let measurement: Measurement | null = null;
let followsTail = true;
let currentProvider = "Coding agent";
/// Everything this coding service said it offers. Its own list, in its own words.
let offeredCommands: SlashCommand[] = [];
/// The subset currently on screen, which is also what the arrow keys move through.
let visibleCommands: SlashCommand[] = [];
let highlighted = 0;
let promptWasSendable = false;
let currentTitle: string | null = null;
let facts: ConversationFacts = NO_FACTS;
let usage: UsageFacts = NO_USAGE;
/// The models this session announced it can switch to, empty when the provider never said.
let switchableModels: string[] = [];
/// The modes this session announced, empty when the choice comes from the service's declared set instead.
let switchableModeIds: string[] = [];
/// Switches sent but not yet confirmed by the provider. Each rides its chip as a "(requested)"
/// suffix and is cleared by the matching confirmation event, or at turn end as the fallback.
let requested = { model: "", mode: "", effort: "" };
/// Messages typed while the agent worked, sent one per turn boundary. This page's memory only.
let queued: readonly string[] = [];
/// An @-mention picker is open in the host; further @ keystrokes wait until it answers.
let mentionPending = false;

prompt.addEventListener("paste", (event) => {
  const images = [...(event.clipboardData?.items ?? [])]
    .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
    .slice(0, 8);
  if (images.length === 0) return;
  event.preventDefault();
  for (const [index, item] of images.entries()) {
    const file = item.getAsFile();
    if (!file) continue;
    void imageAsBase64(file).then((base64Data) => {
      vscode.postMessage({
        type: "pasteImage",
        name: file.name || `pasted-image-${index + 1}.${imageExtension(file.type)}`,
        mediaType: file.type,
        base64Data,
      });
    }).catch(() => {
      setStatus("The pasted image could not be read.", "warning");
    });
  }
});

window.addEventListener("message", ({ data }: MessageEvent<Incoming>) => {
  if (data.type === "reset") {
    // The tab's identity, persisted where VS Code keeps webview state, so a restored tab knows which
    // session (or which draft) it was showing and the extension can rebind it instead of guessing.
    vscode.setState(data.session ? { sessionId: data.session.sessionId } : { draft: data.draftState });
    draft = data.session ? null : data.draft;
    context = null;
    reset(data.session, data.title, data.provider, data.generation);
    return;
  }
  if (data.type === "session") {
    selected = data.session;
    renderSession(data.session, data.title, data.provider);
    return;
  }
  if (data.type === "draft") {
    if (selected) return;
    vscode.setState({ draft: data.draftState });
    draft = data.draft;
    paintFacts();
    paintGreeting();
    return;
  }
  if (data.type === "context") {
    context = data.context;
    paintFacts();
    return;
  }
  if (data.type === "attachments") {
    paintAttachments(data.items);
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
  if (data.type === "switchRequested") {
    requested = { ...requested, [data.what]: data.value };
    paintFacts();
    return;
  }
  if (data.type === "menu") {
    openChipMenu(data.menu, data.anchor, data.title, data.items);
    return;
  }
  if (data.type === "menuClose") {
    if (openMenu?.id === data.menu) hideChipMenu();
    return;
  }
  if (data.type === "clickChip") {
    chipOf(data.anchor)?.click();
    return;
  }
  if (data.type === "openLatestDiff") {
    // The host asking on the operator's behalf (the journey and the eye pass drive it): the newest declared
    // change on the page opens exactly as a click on its button would.
    const buttons = conversation.querySelectorAll<HTMLButtonElement>("button.diff-open");
    buttons[buttons.length - 1]?.click();
    return;
  }
  if (data.type === "insertText") {
    mentionPending = false;
    if (data.text !== null) {
      const caret = prompt.selectionStart ?? prompt.value.length;
      const inserted = insertMention(prompt.value, caret, data.text);
      prompt.value = inserted.value;
      prompt.setSelectionRange(inserted.caret, inserted.caret);
      resizePrompt();
      refreshCommands();
    }
    prompt.focus();
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
  // A draft sends its first message with no session yet; the host starts one with the chips as set.
  if (!selected && !draft) return;
  if (!text.trim() && attachmentList.hidden) {
    return;
  }
  if (!promptWasSendable) {
    // The agent is still working: Enter queues, and the strip above the composer is the receipt.
    const outcome = pushQueued(queued, text);
    if (!outcome.accepted) {
      setStatus(outcome.why, "warning");
      return;
    }
    queued = outcome.queue;
    paintQueued();
  } else {
    vscode.postMessage({ type: "prompt", text });
  }
  prompt.value = "";
  resizePrompt();
  prompt.focus();
});
prompt.addEventListener("keydown", (event) => {
  if (event.isComposing) return;
  // The command menu owns the arrow keys and Enter while it is open, because that is what it is for. Escape
  // closes it and gives the keys back rather than clearing what was typed.
  if (visibleCommands.length > 0) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      highlighted = movedHighlight(highlighted, visibleCommands.length, event.key === "ArrowDown" ? 1 : -1);
      paintCommands();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeCommands();
      return;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      const chosen = visibleCommands[highlighted];
      if (chosen) {
        event.preventDefault();
        chooseCommand(chosen);
        return;
      }
    }
  }
  // Enter sends and Shift+Enter writes a new line, which is what every chat surface has taught people to expect.
  // A modifier-to-send binding makes the common action the awkward one.
  if (event.key !== "Enter" || event.shiftKey || event.altKey) return;
  event.preventDefault();
  composer.requestSubmit();
});
prompt.addEventListener("input", () => {
  resizePrompt();
  refreshCommands();
  // A word-starting @ opens the host's file picker; the chosen path comes back as plain text.
  if (selected && !mentionPending && mentionTriggered(prompt.value, prompt.selectionStart ?? prompt.value.length)) {
    mentionPending = true;
    vscode.postMessage({ type: "mentionFile" });
  }
});
prompt.addEventListener("blur", () => {
  // Only after the click that may have landed on the menu has been delivered.
  setTimeout(closeCommands, 120);
});
interrupt.addEventListener("click", () => vscode.postMessage({ type: "interrupt" }));
// Escape stops the running turn. Document-level because the composer is disabled while the agent
// works, so nothing else in this page can hold focus for the key. Guards, in order: a consumed
// Escape (the command menu closes itself on the composer and prevents default) is not a stop; a
// hidden or disabled stop button means the session is not interruptible right now.
document.addEventListener("keydown", (event) => {
  if (openMenu && !event.isComposing) {
    // The popover owns the keys while it is open: arrows move, Enter chooses, Escape dismisses.
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      openMenu.highlighted = (openMenu.highlighted + step + openMenu.items.length) % openMenu.items.length;
      paintChipMenu();
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      chooseMenuItem(openMenu.items[openMenu.highlighted] ?? null);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      chooseMenuItem(null);
      return;
    }
    return;
  }
  if (event.key !== "Escape" || event.isComposing || event.defaultPrevented) return;
  if (interrupt.hidden || interrupt.disabled) return;
  event.preventDefault();
  vscode.postMessage({ type: "interrupt" });
});
// A click anywhere else dismisses the popover, as every menu does.
document.addEventListener("mousedown", (event) => {
  if (!openMenu) return;
  const target = event.target instanceof Node ? event.target : null;
  if (target && (chipMenu.contains(target) || chipOf(openMenu.anchor)?.contains(target))) return;
  chooseMenuItem(null);
});

function chipOf(anchor: MenuAnchor): HTMLElement | null {
  const id = anchor === "project" ? "project-chip"
    : anchor === "service" ? "service-chip"
      : anchor === "model" ? "model-chip"
        : anchor === "effort" ? "effort-chip"
          : "mode-chip";
  return document.getElementById(id);
}

/// Show the choices for a chip, hanging from that chip, highlighted on the first row.
function openChipMenu(id: string, anchor: MenuAnchor, title: string, items: MenuItem[]): void {
  if (openMenu) hideChipMenu();
  openMenu = { id, anchor, items, highlighted: 0 };
  chipMenu.setAttribute("aria-label", title);
  chipMenu.dataset.anchor = anchor;
  paintChipMenu();
  const chip = chipOf(anchor);
  chip?.setAttribute("aria-expanded", "true");
  const card = chipMenu.parentElement;
  if (chip && card) {
    // Left edge on the chip, clamped inside the card; the bar's chips are near the right edge.
    const cardBox = card.getBoundingClientRect();
    const chipBox = chip.getBoundingClientRect();
    const left = Math.max(0, Math.min(chipBox.left - cardBox.left, cardBox.width - chipMenu.offsetWidth - 8));
    chipMenu.style.left = `${left}px`;
  }
}

function paintChipMenu(): void {
  if (!openMenu) {
    chipMenu.hidden = true;
    chipMenu.replaceChildren();
    return;
  }
  const menu = openMenu;
  chipMenu.replaceChildren(...menu.items.map((item, index) => {
    const row = document.createElement("li");
    row.id = `runtrol-chip-option-${index}`;
    row.className = index === menu.highlighted ? "command command-active" : "command";
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", index === menu.highlighted ? "true" : "false");
    const name = document.createElement("span");
    name.className = "command-name";
    name.textContent = item.label;
    row.append(name);
    if (item.description) {
      const description = document.createElement("span");
      description.className = "command-description";
      description.textContent = item.description;
      row.append(description);
    }
    if (item.detail) {
      const detail = document.createElement("span");
      detail.className = "command-detail";
      detail.textContent = item.detail;
      row.append(detail);
    }
    row.addEventListener("mousedown", (event) => {
      event.preventDefault();
      chooseMenuItem(item);
    });
    row.addEventListener("mousemove", () => {
      if (menu.highlighted !== index) {
        menu.highlighted = index;
        paintChipMenu();
      }
    });
    return row;
  }));
  chipMenu.hidden = false;
  chipOf(menu.anchor)?.setAttribute("aria-activedescendant", `runtrol-chip-option-${menu.highlighted}`);
  chipMenu.querySelector<HTMLElement>(".command-active")?.scrollIntoView({ block: "nearest" });
}

function hideChipMenu(): void {
  const chip = openMenu ? chipOf(openMenu.anchor) : null;
  chip?.setAttribute("aria-expanded", "false");
  chip?.removeAttribute("aria-activedescendant");
  openMenu = null;
  chipMenu.hidden = true;
  chipMenu.replaceChildren();
  chipMenu.style.left = "";
}

function chooseMenuItem(item: MenuItem | null): void {
  const menu = openMenu;
  if (!menu) return;
  const chip = chipOf(menu.anchor);
  hideChipMenu();
  vscode.postMessage({ type: "menuChoice", menu: menu.id, choice: item ? item.id : null });
  // A completed choice flows into writing the message. Escape returns to the control it dismissed instead of
  // moving keyboard focus somewhere unrelated.
  (item ? prompt : chip)?.focus();
}
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
  // The queue lives exactly as long as this page's view of one session. Anything still waiting when
  // the session changes was addressed to the previous one.
  queued = [];
  paintQueued();
  promptWasSendable = false;
  renderSession(session, displayTitle, provider);
  prompt.value = "";
  resizePrompt();
  paintAttachments([]);
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
  document.body.classList.toggle("no-chat", !session && !draft);
  document.body.classList.toggle("drafting", !session && draft !== null);
  document.body.classList.toggle("opening", session?.lifecycle === "cold");
  document.body.classList.toggle("working", session?.lifecycle === "hotRunning");
  document.body.classList.toggle("throttled", session?.waitingOn === "quota");
  facts = { ...facts, service: session ? currentProvider : "" };
  paintFacts();
  // A draft is always sendable: its first message is what starts the conversation.
  const canSend = session ? session.lifecycle === "hotIdle" : draft !== null;
  const canInterrupt = session?.lifecycle === "hotRunning";
  // A turn that stopped for a person is still a running turn, so the lifecycle alone would say "working" about
  // the one session that is actually waiting on the reader.
  const waitingOnYou = session?.waitingOn === "person";
  document.body.classList.toggle("waiting", waitingOnYou);
  // Typing stays open while the agent works: Enter then queues instead of sending, because a person
  // watching a turn is already composing the next message. Every other unsendable state keeps the
  // composer closed.
  prompt.disabled = !canSend && !canInterrupt;
  send.disabled = !canSend;
  send.hidden = !canSend;
  sendHint.hidden = !canSend;
  interrupt.disabled = !canInterrupt;
  interrupt.hidden = !canInterrupt;
  // Images ride with a message, so they can be added whenever one can be written.
  attach.disabled = !canSend && !canInterrupt;
  prompt.setAttribute("aria-label", session ? `Message ${currentProvider}` : "Message");
  prompt.placeholder = !session
    ? draft
      ? "Ask anything"
      : "Message"
    : canSend
      ? `Message ${currentProvider}`
      : waitingOnYou
        ? `${currentProvider} is waiting for you`
        : session.waitingOn === "quota"
          ? `${currentProvider} is waiting on an account limit`
          : canInterrupt
            ? `Message ${currentProvider}. Enter queues it for the end of this turn`
            : session.lifecycle === "failed"
              ? "This conversation needs attention"
              : "Paused. Open it again from the sidebar to continue";
  if (canSend && !promptWasSendable) {
    // The turn just ended. One queued message goes now (one per transition, so an answer arrives
    // between messages exactly as if the person had typed each one at the moment it became possible).
    const taken = takeQueued(queued);
    if (taken.next !== null) {
      queued = taken.queue;
      paintQueued();
      vscode.postMessage({ type: "prompt", text: taken.next });
    }
    if (document.hasFocus()) {
      prompt.focus();
    }
  }
  promptWasSendable = canSend;
}

/// The strip of messages waiting for their turn, each with its own cancel.
function paintQueued(): void {
  if (queued.length === 0) {
    queuedList.hidden = true;
    queuedList.replaceChildren();
    return;
  }
  queuedList.replaceChildren(...queued.map((text, index) => {
    const row = document.createElement("li");
    row.className = "queued-message";
    const label = document.createElement("span");
    label.className = "queued-text";
    label.textContent = queuedLabel(text);
    label.title = "Sends when this turn ends";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "queued-cancel";
    cancel.textContent = "×";
    cancel.title = "Do not send this";
    cancel.setAttribute("aria-label", "Remove the queued message");
    cancel.addEventListener("click", () => {
      queued = cancelQueued(queued, index);
      paintQueued();
    });
    row.append(label, cancel);
    return row;
  }));
  queuedList.hidden = false;
}

/// The chips around the composer: where the conversation runs, who answers, and how.
///
/// A draft paints its choices (each chip is the picker for that choice); a live conversation paints what
/// the host and the service said. Either way the chip is the place that says the fact and the place to
/// change it, and a chip with nothing true to say stays hidden rather than guessing.
function paintFacts(): void {
  const place = draft ?? context;
  setChip(projectChip, place?.project ?? "");
  setChip(branchChip, place?.branch ?? "");
  projectChip.title = place?.projectPath ?? "Project";
  setChip(serviceChip, draft ? draft.service : facts.service);
  setChip(modelChip, draft ? draft.model : modelLine(facts, requested.model) || "Model");
  setChip(effortChip, draft ? draft.effort : chipText(facts.effort, requested.effort) || "Effort");
  setChip(modeChip, draft ? draft.mode : chipText(facts.mode, requested.mode));
  const spent = usageLine(usage, Date.now());
  usageChip.textContent = spent;
  usageChip.hidden = !spent;
  usageChip.classList.toggle("limit-reached", usage.reached);
}

function setChip(chip: HTMLElement, text: string): void {
  chip.textContent = text;
  chip.hidden = !text;
}

/// The greeting of a draft, redrawn when its project changes (the words name the project).
function paintGreeting(): void {
  if (!draft || selected) return;
  const heading = conversation.querySelector<HTMLElement>("[data-placeholder='true'] h1");
  if (heading) heading.textContent = draftGreeting(draft);
}

/// The images waiting to ride with the next message, each with its own remove.
function paintAttachments(items: readonly AttachmentLabel[]): void {
  if (items.length === 0) {
    attachmentList.hidden = true;
    attachmentList.replaceChildren();
    return;
  }
  attachmentList.replaceChildren(...items.map((item, index) => {
    const row = document.createElement("li");
    row.className = "attachment";
    const label = document.createElement("span");
    label.className = "attachment-name";
    label.textContent = `${item.name} · ${item.kilobytes} KB`;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "attachment-remove";
    remove.textContent = "×";
    remove.title = "Remove this image";
    remove.setAttribute("aria-label", `Remove ${item.name}`);
    remove.addEventListener("click", () => vscode.postMessage({ type: "removeAttachment", index }));
    row.append(label, remove);
    return row;
  }));
  attachmentList.hidden = false;
}

function renderEmptyState(session: Session | null): void {
  const empty = document.createElement("section");
  empty.dataset.placeholder = "true";
  empty.dataset.characters = "0";
  const mark = document.createElement("div");
  mark.className = "empty-mark";
  mark.textContent = "R";
  const heading = document.createElement("h1");
  const detail = document.createElement("p");
  if (!session && draft) {
    // A conversation about to begin: the greeting the chat apps use, naming the project, and nothing else
    // in the way of the composer.
    empty.className = "empty-state empty-hero empty-draft";
    heading.textContent = draftGreeting(draft);
    detail.textContent = "";
    empty.append(mark, heading);
    conversation.append(empty);
    return;
  }
  const emptyCopy = conversationEmptyCopy(session, currentProvider, currentTitle);
  empty.className = `empty-state empty-${emptyCopy.tone}`;
  heading.textContent = emptyCopy.heading;
  detail.textContent = emptyCopy.detail;
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
  if (presentation.kind === "tool") {
    appendTool(body, presentation.part);
    return;
  }
  if (presentation.kind === "turn") {
    if (string(body.step) === "ended") {
      // In-flight switches do not outlive the turn. Model and mode have confirmation events, so an
      // unconfirmed request reverts honestly here. The effort never gets a confirmation (no CLI
      // sends one), so the accepted request becomes the displayed value, exactly as the attach-time
      // effort was itself only ever the requested one.
      if (requested.effort) facts = { ...facts, effort: requested.effort };
      requested = { model: "", mode: "", effort: "" };
      paintFacts();
    }
    // A turn beginning and ending is already visible: the composer swaps its button and the row changes state.
    // Printing "Turn started" into the transcript says what the reader can already see, and it says it in the
    // protocol's words. Only an ending that stopped for a reason worth acting on earns a line.
    appendTurnEnd(body);
    return;
  }
  if (presentation.kind === "notice") {
    appendNotice(body);
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
    if (presentation.textKey === "session.attached") {
      updateAttachment(body);
      // Claude Code names its slash commands inside the frame it says hello with, not in a later update, and the
      // driver carries that frame through whole. So for the flagship service the list is already here, and
      // services that announce separately still overwrite it below.
      adoptCommands(body, false);
    }
    if (presentation.textKey === "mode.updated") updateMode(body);
    if (presentation.textKey === "model.updated") updateModel(body);
    // A plan with readable entries is shown as the checklist the service sent. Only a plan event
    // without one falls through to the one-line notice, because inventing entries would be worse
    // than saying "Plan updated".
    if (presentation.textKey === "plan.updated" && renderPlan(body)) return;
    // Kept out of the transcript below and remembered here. "The command list changed" is not something anybody
    // came to read, but the list itself is the only way to find out what this CLI can be told to do.
    if (presentation.textKey === "commands.updated") adoptCommands(body, true);
    const text = LOCALIZED_TEXT[presentation.textKey];
    if (text && !HIDDEN_STATUS_KEYS.has(presentation.textKey)) appendMessage("meta", text);
  }
}

/// Remember what this coding service says it offers.
///
/// Replaces rather than merges. The service is the authority on its own command list, and a list that only ever
/// grew would keep offering a command that a mode change had withdrawn.
///
/// `authoritative` separates the two ways a list arrives. A dedicated command update is the service saying what it
/// offers now, so an empty one withdraws everything. A startup frame is the service saying hello, and the ones that
/// mention commands there do it in passing: reading an absence as a withdrawal would clear a list that a later
/// update had already delivered.
function adoptCommands(body: UnknownRecord, authoritative: boolean): void {
  const announced = slashCommandsOf(body);
  if (announced.length === 0 && !authoritative) return;
  offeredCommands = announced;
  refreshCommands();
}

/// Show the candidates for what is typed, or nothing.
function refreshCommands(): void {
  const next = selected ? matchingCommands(offeredCommands, prompt.value) : [];
  // Keeps the highlight on the same command when the list narrows under it, instead of snapping to the top.
  const previous = visibleCommands[highlighted]?.name;
  visibleCommands = next;
  const stayed = next.findIndex((command) => command.name === previous);
  highlighted = stayed >= 0 ? stayed : 0;
  paintCommands();
}

function paintCommands(): void {
  if (visibleCommands.length === 0) {
    commandMenu.hidden = true;
    commandMenu.replaceChildren();
    prompt.setAttribute("aria-expanded", "false");
    prompt.removeAttribute("aria-activedescendant");
    return;
  }
  commandMenu.replaceChildren(...visibleCommands.map((command, index) => {
    const row = document.createElement("li");
    row.id = `runtrol-command-option-${index}`;
    row.className = index === highlighted ? "command command-active" : "command";
    row.setAttribute("role", "option");
    row.setAttribute("aria-selected", index === highlighted ? "true" : "false");
    const name = document.createElement("span");
    name.className = "command-name";
    name.textContent = `/${command.name}`;
    row.append(name);
    if (command.description) {
      const description = document.createElement("span");
      description.className = "command-description";
      description.textContent = command.description;
      row.append(description);
    }
    // Pointer rather than click, so the choice lands before the textarea's blur can close the menu.
    row.addEventListener("mousedown", (event) => {
      event.preventDefault();
      chooseCommand(command);
    });
    return row;
  }));
  commandMenu.hidden = false;
  prompt.setAttribute("aria-expanded", "true");
  prompt.setAttribute("aria-activedescendant", `runtrol-command-option-${highlighted}`);
}

function imageAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("load", () => {
      const value = reader.result;
      if (typeof value !== "string") {
        reject(new Error("the clipboard image had no data URL"));
        return;
      }
      const comma = value.indexOf(",");
      if (comma < 0) {
        reject(new Error("the clipboard image data URL was malformed"));
        return;
      }
      resolve(value.slice(comma + 1));
    }, { once: true });
    reader.addEventListener("error", () => reject(reader.error ?? new Error("clipboard read failed")), { once: true });
    reader.readAsDataURL(file);
  });
}

function imageExtension(mediaType: string): string {
  if (mediaType === "image/jpeg") return "jpg";
  if (mediaType === "image/gif") return "gif";
  if (mediaType === "image/webp") return "webp";
  return "png";
}

function closeCommands(): void {
  if (visibleCommands.length === 0) return;
  visibleCommands = [];
  highlighted = 0;
  paintCommands();
}

/// Put the chosen command in the composer and leave it there.
///
/// Never sends it. A command is a message the person is composing, and some of them take an argument; sending on
/// selection would make the ones that do unusable and would send the others before anybody meant to.
function chooseCommand(command: SlashCommand): void {
  prompt.value = completed(command);
  closeCommands();
  resizePrompt();
  prompt.focus();
}

function resetSessionTelemetry(): void {
  facts = { ...NO_FACTS, service: currentProvider };
  usage = NO_USAGE;
  switchableModels = [];
  requested = { model: "", mode: "", effort: "" };
  paintFacts();
  // A command list belongs to one conversation. Carrying it across would offer the previous service's commands
  // to the next one, and the wrong list is worse than none: it looks authoritative.
  offeredCommands = [];
  closeCommands();
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
  // The provider's word arrived; the request is no longer in flight, whatever it asked for.
  requested = { ...requested, mode: "" };
  if (Array.isArray(body.available_ids)) {
    switchableModeIds = body.available_ids.filter(
      (id): id is string => typeof id === "string" && id.length > 0,
    );
  }
  paintFacts();
}

/// The provider's own word on which model answers now, and which ones this session may switch to.
function updateModel(body: UnknownRecord): void {
  facts = { ...facts, model: string(body.model_id) };
  requested = { ...requested, model: "" };
  if (Array.isArray(body.available_ids)) {
    switchableModels = body.available_ids.filter(
      (id): id is string => typeof id === "string" && id.length > 0,
    );
  }
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
  // The whole of a message that was already streamed piece by piece. Claude Code sends both: the deltas as
  // they happen and the assembled message after (measured in the real window: the reply appeared twice,
  // once in pieces and once whole). The whole replaces what the pieces built, because it is the same message
  // and the provider's final word on it; a second whole with the same name is a further block of the same
  // message and is appended like any other.
  if (!delta && messageId) {
    const streamed = conversation.querySelector<HTMLElement>(
      `[data-message-id="${cssEscape(messageId)}"][data-side="${cssEscape(side)}"][data-streamed="1"]`,
    );
    if (streamed) {
      visibleCharacters -= Number(streamed.dataset.characters ?? 0);
      delete streamed.dataset.streamed;
      streamed.replaceChildren();
      if (side === "theirs") {
        streamed.dataset.raw = text;
        if (hasMarkdownTrigger(text)) {
          streamed.dataset.md = "1";
          repaintMarkdown(streamed, text);
        } else {
          delete streamed.dataset.md;
          appendAuthorAndText(streamed, side, text);
        }
      } else {
        appendAuthorAndText(streamed, side, text);
      }
      streamed.dataset.characters = String(text.length);
      visibleCharacters += text.length;
      trim();
      return;
    }
  }
  if (
    delta
    && messageId
    && last?.dataset.messageId === messageId
    && last.dataset.side === side
    && lastCharacters + text.length <= MAX_MESSAGE_CHARACTERS
  ) {
    if (side === "theirs") {
      // The raw text rides on the element so a trigger arriving mid-stream can re-read everything
      // said so far. Scanning the whole raw rather than just the delta is what catches a marker
      // split across two chunks, and the raw is bounded by the same per-element ceiling as the text.
      const raw = (last.dataset.raw ?? "") + text;
      last.dataset.raw = raw;
      if (last.dataset.md === "1" || hasMarkdownTrigger(raw)) {
        last.dataset.md = "1";
        repaintMarkdown(last, raw);
      } else {
        appendTailText(last, text);
      }
    } else {
      appendTailText(last, text);
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
  // Built from pieces: the whole, if it comes, replaces this rather than repeating it.
  if (delta) item.dataset.streamed = "1";
  item.dataset.characters = String(text.length);
  if (side === "theirs") {
    item.dataset.raw = text;
    if (hasMarkdownTrigger(text)) {
      item.dataset.md = "1";
      repaintMarkdown(item, text);
    } else {
      appendAuthorAndText(item, side, text);
    }
  } else {
    appendAuthorAndText(item, side, text);
  }
  visibleCharacters += text.length;
  conversation.append(item);
  trim();
}

function appendAuthorAndText(item: HTMLElement, side: string, text: string): void {
  const author = messageAuthor(side);
  if (author) {
    const label = document.createElement("span");
    label.className = "message-author";
    label.textContent = author;
    item.append(label);
  }
  item.append(document.createTextNode(text));
}

function appendTailText(item: HTMLElement, text: string): void {
  const tail = item.lastChild;
  if (tail instanceof Text) {
    tail.appendData(text);
  } else {
    item.append(document.createTextNode(text));
  }
}

/// The agent's reply, re-read as a whole and repainted.
///
/// A whole-element repaint per delta is deliberate: an element holds at most MAX_MESSAGE_CHARACTERS,
/// so the parse is bounded, and messages without any markdown never reach here (the trigger check is
/// the hot path). Every leaf below is a text node; no conversation byte ever becomes markup.
function repaintMarkdown(item: HTMLElement, raw: string): void {
  const nodes: Node[] = [];
  const author = messageAuthor("theirs");
  if (author) {
    const label = document.createElement("span");
    label.className = "message-author";
    label.textContent = author;
    nodes.push(label);
  }
  for (const block of parseMarkdown(raw)) {
    nodes.push(blockNode(block));
  }
  item.replaceChildren(...nodes);
}

function blockNode(block: Block): Node {
  if (block.kind === "codeBlock") {
    const wrap = document.createElement("div");
    wrap.className = "md-code";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "code-copy";
    copy.textContent = "Copy";
    copy.title = "Copy code";
    const pre = document.createElement("pre");
    const code = document.createElement("code");
    code.textContent = block.text;
    pre.append(code);
    copy.addEventListener("click", () => {
      // Failure is silent by design: the button is a convenience over selectable text that is
      // already on screen, and a clipboard refusal must not interrupt reading.
      void navigator.clipboard.writeText(code.textContent ?? "").catch(() => undefined);
    });
    wrap.append(copy, pre);
    return wrap;
  }
  if (block.kind === "heading") {
    const level = Math.min(6, Math.max(1, block.level));
    const heading = document.createElement(`h${level}`);
    heading.className = "md-heading";
    heading.append(...inlineNodes(block.inlines));
    return heading;
  }
  if (block.kind === "list") {
    const list = document.createElement(block.ordered ? "ol" : "ul");
    list.className = "md-list";
    for (const item of block.items) {
      const row = document.createElement("li");
      row.append(...inlineNodes(item));
      list.append(row);
    }
    return list;
  }
  const paragraph = document.createElement("p");
  paragraph.className = "md-paragraph";
  paragraph.append(...inlineNodes(block.inlines));
  return paragraph;
}

function inlineNodes(inlines: Inline[]): Node[] {
  return inlines.map((inline) => {
    if (inline.kind === "code") {
      const code = document.createElement("code");
      code.textContent = inline.text;
      return code;
    }
    if (inline.kind === "strong") {
      const strong = document.createElement("strong");
      strong.textContent = inline.text;
      return strong;
    }
    if (inline.kind === "em") {
      const em = document.createElement("em");
      em.textContent = inline.text;
      return em;
    }
    if (inline.kind === "link") {
      // The parser only produces http(s) addresses, and the webview's own link handler opens them
      // in the external browser. The full address rides on the tooltip so the text cannot disguise
      // the destination.
      const anchor = document.createElement("a");
      anchor.href = inline.href;
      anchor.title = inline.href;
      anchor.textContent = inline.text;
      return anchor;
    }
    return document.createTextNode(inline.text);
  });
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
/// One tool call, as a heading that can be opened to see what the agent actually sent and got back.
///
/// The details are folded away rather than dropped. Watching an agent work means being able to check what it did,
/// and a surface that shows only "Edit src/main.rs" makes the reader open their own editor to find out. The thin
/// principle forbids storing, interpreting or rewriting a conversation; showing one is the entire point of the
/// product, and this panel keeps nothing after the row scrolls away.
function appendTool(body: UnknownRecord, part: "call" | "result"): void {
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

  const activity = toolActivityOf(body);
  const line = toolActivityLineKeeping(activity, item.dataset.toolLabel ?? "");
  if (activity.target) item.dataset.toolLabel = activity.target;

  item.className = `message tool tool-${activity.state}`;
  visibleCharacters -= Number(item.dataset.characters ?? 0);

  // What went in and what came back sit in one folded block, which is how the coding services show their own
  // tools. Two slots rather than one because an update replacing the call would erase the arguments the moment
  // the result arrived, and rather than a growing list because a service streams many updates per call.
  const slots = new Map<string, string>();
  for (const pre of item.querySelectorAll("pre.tool-detail")) {
    slots.set(pre.getAttribute("data-slot") ?? IN, pre.textContent ?? "");
  }
  const slot = part === "result" ? OUT : IN;
  // A change the service declared as a change is named once, with a button that opens it in VS Code's own
  // diff editor: the raw detail skips only the keys whose every entry that row covered. Rows persist per
  // slot like the raw detail, because a later frame for the same call may not repeat the change it declared
  // earlier.
  const declared = declaredDiffs(body);
  const diffBlocks = new Map<string, HTMLElement>();
  for (const block of item.querySelectorAll<HTMLElement>("div.tool-diffs")) {
    diffBlocks.set(block.getAttribute("data-slot") ?? IN, block);
  }
  if (declared.diffs.length > 0) {
    diffBlocks.set(slot, diffContainer(declared.diffs, slot));
  }
  const detail = toolDetail(body, declared.consumed);
  if (detail) slots.set(slot, detail);

  if (slots.size === 0 && diffBlocks.size === 0) {
    item.replaceChildren(document.createTextNode(line));
    item.dataset.characters = String(line.length);
    visibleCharacters += line.length;
    trim();
    return;
  }
  // An open panel stays open while its call updates, because a reader watching output arrive should not have to
  // reopen it every time another chunk lands.
  const wasOpen = item.querySelector("details")?.open ?? false;
  const panel = document.createElement("details");
  panel.open = wasOpen;
  const summary = document.createElement("summary");
  summary.textContent = line;
  panel.append(summary);
  let length = line.length;
  for (const side of [IN, OUT]) {
    const block = diffBlocks.get(side);
    if (block) {
      panel.append(block);
      length += block.textContent?.length ?? 0;
    }
    const text = slots.get(side);
    if (!text) continue;
    const pre = document.createElement("pre");
    pre.className = "tool-detail";
    pre.setAttribute("data-slot", side);
    pre.textContent = text;
    panel.append(pre);
    length += text.length;
  }
  item.replaceChildren(panel);
  item.dataset.characters = String(length);
  visibleCharacters += length;
  trim();
}

/// Declared changes, one row each: the path the service named and a button that opens the change in VS
/// Code's own diff editor. The page draws no diff and colours nothing; the editor that already knows how to
/// show a change shows it, which is also how the Codex and Claude apps hand a change to the reader.
function diffContainer(diffs: DeclaredDiff[], slot: string): HTMLElement {
  const container = document.createElement("div");
  container.className = "tool-diffs";
  container.setAttribute("data-slot", slot);
  for (const diff of diffs) {
    const row = document.createElement("div");
    row.className = "tool-diff";
    const path = document.createElement("span");
    path.className = "diff-path";
    path.textContent = diff.path || "(a change with no path)";
    const measure = document.createElement("span");
    measure.className = "diff-measure";
    measure.textContent = diffMeasure(diff);
    const open = document.createElement("button");
    open.type = "button";
    open.className = "diff-open";
    open.textContent = "Open diff";
    open.title = "Open this change in the diff editor";
    open.addEventListener("click", () => {
      vscode.postMessage({ type: "openDiff", diff });
    });
    row.append(path, measure, open);
    container.append(row);
  }
  return container;
}

/// What the row says about the size of a change, read from nothing but the change's own text.
function diffMeasure(diff: DeclaredDiff): string {
  if (diff.kind === "unified") {
    let added = 0;
    let removed = 0;
    for (const lineText of diff.text.split("\n")) {
      if (lineText.startsWith("+") && !lineText.startsWith("+++")) added += 1;
      else if (lineText.startsWith("-") && !lineText.startsWith("---")) removed += 1;
    }
    return `+${added} -${removed}`;
  }
  const before = diff.oldText ? diff.oldText.split("\n").length : 0;
  const after = diff.newText ? diff.newText.split("\n").length : 0;
  return `${before} -> ${after} lines`;
}

/// The plan as the service last announced it: one element, updated in place.
///
/// The service resends the whole plan on every change (that is the ACP shape), so the session keeps a
/// single plan element rather than a growing history of checklists.
function renderPlan(body: UnknownRecord): boolean {
  const entries = planEntriesOf(body);
  if (entries.length === 0) return false;
  clearPlaceholder();
  const existing = conversation.querySelector<HTMLElement>("[data-plan='true']");
  const item = existing ?? document.createElement("article");
  if (!existing) {
    item.className = "message plan";
    item.dataset.plan = "true";
    conversation.append(item);
  }
  visibleCharacters -= Number(item.dataset.characters ?? 0);
  const label = document.createElement("span");
  label.className = "message-author";
  label.textContent = "Plan";
  const list = document.createElement("ul");
  list.className = "plan-entries";
  let length = label.textContent.length;
  for (const entry of entries) {
    const row = document.createElement("li");
    row.className = `plan-entry plan-${entry.status}`;
    const glyph = document.createElement("span");
    glyph.className = "plan-glyph";
    glyph.textContent = planGlyph(entry.status);
    row.append(glyph, document.createTextNode(entry.content));
    list.append(row);
    length += entry.content.length + 1;
  }
  item.replaceChildren(label, list);
  item.dataset.characters = String(length);
  visibleCharacters += length;
  trim();
  return true;
}

/// The call's own arguments, as the service sent them.
const IN = "in";
/// What came back, as the service sent it.
const OUT = "out";

/// A turn ending, only when the ending says something the reader would act on.
///
/// A turn starting and finishing is already visible: the composer swaps its send button for a stop button and the
/// row in the sidebar changes state. Printing "Turn started" restates that in the protocol's vocabulary. An ending
/// that is anything other than the agent finishing normally is worth a line, because that is the case where the
/// reader would otherwise wonder why the reply stopped.
function appendTurnEnd(body: UnknownRecord): void {
  if (string(body.step) !== "ended") return;
  const stop = string(body.stop);
  if (!stop || stop === "endTurn") return;
  appendMessage("meta", stop);
}

/// A notice from the coding service, in the service's own words or not at all.
///
/// The code is a protocol value. Printing it put the literal word "other" in the transcript five times, which told
/// the reader nothing and looked like an error because it was styled as a warning. Only a notice carrying text a
/// person can read earns a line, and its level decides how loud it looks.
function appendNotice(body: UnknownRecord): void {
  const text = string(body.message) || string(body.detail) || textOf(record(body.payload)?.message);
  if (!text.trim()) return;
  appendMessage(string(body.level) === "error" ? "warning" : "meta", text.trim());
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
