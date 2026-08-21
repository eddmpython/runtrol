import * as vscode from "vscode";

import { type ConversationSurface, type Place, tabSurface } from "./conversationSurface";
import type { DraftChips, DraftState } from "./draft";
import type { SessionLine } from "./runtimeTypes";
import { providerDisplayName, sessionTitle } from "./sessionDisplay";
import { type MenuAnchor, type MenuItem, MAX_MENU_ITEMS, isViewAction, type ViewAction } from "./viewActions";
import { webviewReadyKind } from "./webviewReady";

export type WebviewPerformance = {
  baselineFrameP95Ms: number;
  frameP95Ms: number;
  frameOverrunP95Ms: number;
  inputP95Ms: number;
  scrollP95Ms: number;
  maxPendingFrames: number;
  producedFrames: number;
  droppedFrames: number;
  visibleCharacters: number;
  visibleItems: number;
};

type FrameEnvelope = {
  generation: number;
  payload: unknown;
};

/// Where a live conversation runs, as the chips say it.
export type ConversationContext = {
  readonly project: string;
  readonly projectPath: string | null;
  readonly branch: string | null;
};

/// One attachment as the page lists it: a name and a size, never the bytes.
export type AttachmentLabel = {
  readonly name: string;
  readonly kilobytes: number;
};

type MeasurementWaiter = {
  ready: Promise<void>;
  readyResolve(): void;
  result: Promise<WebviewPerformance>;
  resultResolve(value: WebviewPerformance): void;
  reject(error: Error): void;
};

type RenderWaiter = {
  generation: number;
  resolve(): void;
  reject(error: Error): void;
};

const MAX_PENDING_POSTS = 4_096;
const POST_BATCH = MAX_PENDING_POSTS;
const VISIBLE_READY_TIMEOUT_MS = 5_000;
const VISIBLE_READY_RELOAD_MS = 1_500;
const MEASUREMENT_ATTEMPTS = 2;
const MEASUREMENT_STAGE_TIMEOUT_MS = 5_000;

class RetryableMeasurementError extends Error {}

export class ConversationView implements vscode.Disposable {
  static readonly viewType = "runtrol.conversation";
  /// The place this conversation is shown in right now (a tab, the panel, the side bar), or none.
  private surface: ConversationSurface | null = null;
  /// Listeners on the current surface, dropped when it is replaced or goes away. A view surface outlives the
  /// conversations shown in it, so its listeners cannot be left to the surface's own disposal.
  private surfaceGuards: vscode.Disposable[] = [];
  private selected: SessionLine | null = null;
  /// The draft this tab shows while no session exists yet, with the record the page stamps into its state.
  private draft: { chips: DraftChips; state: DraftState } | null = null;
  private generation = 0;
  private pendingFrames: FrameEnvelope[] = [];
  private posting: Promise<void> | null = null;
  private postGap = false;
  private droppedFrames = 0;
  private readonly measurements = new Map<string, MeasurementWaiter>();
  private readonly renderWaiters = new Set<RenderWaiter>();
  private renderedGeneration = 0;
  private visibleReady = false;
  private showQueue: Promise<void> = Promise.resolve();
  /// The popover the page is showing for a chip, and who is waiting for its answer. One at a time: a new
  /// question closes the old one with no answer.
  private pendingMenu: { id: string; resolve(choice: string | null): void } | null = null;
  private menuSerial = 0;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly action: (message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string = sessionTitle,
    private readonly visibility: (visible: boolean) => void = () => {},
    private readonly providerOf: (session: SessionLine) => string = (session) => providerDisplayName(session.providerId),
  ) {}

  get isOpen(): boolean {
    return this.surface !== null;
  }

  /// Where this conversation is shown, or null while it has no surface.
  get place(): Place | null {
    return this.surface?.place ?? null;
  }

  /// On screen with a webview that said hello, which is when frames can land.
  get isVisible(): boolean {
    return this.surface?.visible === true && this.visibleReady;
  }

  /// Show this conversation on a surface somebody else made: a tab VS Code restored, or one of the
  /// workbench's views. A previous surface closes (a tab) or is emptied (a view).
  adopt(surface: ConversationSurface): Promise<void> {
    const pending = this.showQueue.then(() => {
      this.attach(surface);
    });
    this.showQueue = pending.catch(() => undefined);
    return pending;
  }

  /// Move a tab to an editor column, leaving focus where it is. A view has no column and stays.
  revealIn(column: vscode.ViewColumn): void {
    if (this.surface?.place === "tab") this.surface.reveal(true, column);
  }

  async show(preserveFocus = false): Promise<void> {
    const pending = this.showQueue.then(() => this.showNow(preserveFocus));
    this.showQueue = pending.catch(() => undefined);
    return pending;
  }

  private async showNow(preserveFocus: boolean): Promise<void> {
    const surface = this.surface ?? this.createTab();
    if (preserveFocus) {
      if (!surface.visible) {
        surface.reveal(true);
      }
    } else {
      await focusSurface(surface);
    }
    await this.waitForVisibleWebview(surface);
  }

  /// A fresh editor tab, the default place: beside whatever is open, like a file.
  private createTab(): ConversationSurface {
    const panel = vscode.window.createWebviewPanel(
      ConversationView.viewType,
      this.panelTitle(this.selected),
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "dist")],
        retainContextWhenHidden: false,
      },
    );
    const surface = tabSurface(panel, vscode.Uri.joinPath(this.extensionUri, "resources", "symbol.svg"));
    this.attach(surface);
    return surface;
  }

  private attach(surface: ConversationSurface): void {
    if (this.surface && this.surface !== surface) {
      this.dropSurfaceGuards();
      this.surface.dispose();
    }
    this.surface = surface;
    this.visibleReady = false;
    surface.title = this.panelTitle(this.selected);
    const guards = this.surfaceGuards;
    guards.push(surface.webview.onDidReceiveMessage((message: unknown) => {
      const ready = webviewReadyKind(message);
      if (ready) {
        if (ready === "startup" && this.measurements.size > 0) {
          this.rejectMeasurements(new RetryableMeasurementError(
            "the Runtrol Webview reloaded during measurement",
          ));
        }
        const becameReady = !this.visibleReady;
        this.visibleReady = true;
        if (ready === "startup" || becameReady) {
          this.reset(this.selected, this.draft);
          this.visibility(true);
        }
        return;
      }
      if (this.receiveRenderedGeneration(message)) {
        return;
      }
      if (this.receiveMeasurement(message)) {
        return;
      }
      if (!isViewAction(message)) {
        return;
      }
      if (message.type === "menuChoice") {
        this.receiveMenuChoice(message.menu, message.choice);
        return;
      }
      this.action(message);
    }));
    guards.push(surface.onDidChangeVisibility(() => {
      if (surface.visible || this.surface !== surface) {
        return;
      }
      this.visibleReady = false;
      this.visibility(false);
      this.closeMenu();
      this.rejectMeasurements(new RetryableMeasurementError(
        "the Runtrol Webview became hidden during measurement",
      ));
    }));
    guards.push(surface.onDidDispose(() => {
      if (this.surface === surface) {
        this.dropSurfaceGuards();
        this.surface = null;
        this.visibleReady = false;
        this.visibility(false);
        this.pendingFrames = [];
        this.rejectMeasurements(new Error("the Runtrol Webview closed during measurement"));
        this.rejectRenderWaiters(new Error("the Runtrol Webview closed before painting the selected session"));
      }
    }));
    surface.webview.html = this.html(surface.webview);
  }

  private dropSurfaceGuards(): void {
    for (const guard of this.surfaceGuards) guard.dispose();
    this.surfaceGuards = [];
  }

  /// Show one session, or one draft (a conversation that has not started), in a fresh document.
  reset(
    session: SessionLine | null,
    draft: { chips: DraftChips; state: DraftState } | null = null,
  ): number {
    this.selected = session;
    this.draft = session ? null : draft;
    if (this.surface) {
      this.surface.title = this.panelTitle(session);
    }
    this.generation += 1;
    this.pendingFrames = [];
    this.postGap = false;
    this.closeMenu();
    void this.surface?.webview.postMessage({
      type: "reset",
      session,
      title: session ? this.titleOf(session) : null,
      provider: session ? this.providerOf(session) : null,
      generation: this.generation,
      draft: this.draft?.chips ?? null,
      draftState: this.draft?.state ?? null,
    });
    return this.generation;
  }

  /// The draft's chips changed (a project, service, model, effort or mode was picked).
  updateDraft(chips: DraftChips, state: DraftState): void {
    if (this.selected) return;
    this.draft = { chips, state };
    void this.surface?.webview.postMessage({ type: "draft", draft: chips, draftState: state });
  }

  /// Where a live conversation runs, for the chips above the composer: its folder and branch.
  updateContext(context: ConversationContext): void {
    void this.surface?.webview.postMessage({ type: "context", context });
  }

  /// The images waiting to ride with the next message, as names the page lists above the field.
  updateAttachments(items: readonly AttachmentLabel[]): void {
    void this.surface?.webview.postMessage({ type: "attachments", items });
  }

  updateSession(session: SessionLine): void {
    this.selected = session;
    if (this.surface) {
      this.surface.title = this.panelTitle(session);
    }
    void this.surface?.webview.postMessage({
      type: "session",
      session,
      title: this.titleOf(session),
      provider: this.providerOf(session),
    });
  }

  async waitForCurrentRender(): Promise<void> {
    await this.show(true);
    const generation = this.generation;
    if (this.renderedGeneration >= generation) {
      return;
    }
    return new Promise<void>((resolve, reject) => {
      this.renderWaiters.add({ generation, resolve, reject });
    });
  }

  frame(payload: unknown): boolean {
    if (!this.surface?.visible || !this.visibleReady) {
      return false;
    }
    if (this.pendingFrames.length >= MAX_PENDING_POSTS) {
      const dropped = this.pendingFrames.length - MAX_PENDING_POSTS + 1;
      this.pendingFrames.splice(0, dropped);
      this.droppedFrames += dropped;
      this.postGap = true;
    }
    this.pendingFrames.push({ generation: this.generation, payload });
    this.schedulePosts();
    return true;
  }

  status(message: string, kind: "info" | "warning" | "error" = "info"): void {
    void this.surface?.webview.postMessage({ type: "status", message, kind });
  }

  /// A switch was sent to the provider and is not yet confirmed. The chip shows the request as a
  /// suffix beside the confirmed value; the matching confirmation event clears it.
  switchRequested(what: "model" | "mode" | "effort", value: string): void {
    void this.surface?.webview.postMessage({ type: "switchRequested", what, value });
  }

  /// Ask the page to offer choices in a popover hanging from a chip, where the click was, and wait for the
  /// answer: the chosen item's id, or null when the reader dismissed it. The composer is where the question
  /// was asked, so that is where it is answered (the Codex and ChatGPT composers do the same); the command
  /// palette stays the path for a command invoked from the palette.
  showMenu(anchor: MenuAnchor, title: string, items: readonly MenuItem[]): Promise<string | null> {
    this.closeMenu();
    if (!this.surface || !this.visibleReady) return Promise.resolve(null);
    this.menuSerial += 1;
    const id = `menu-${this.menuSerial}`;
    const offered = items.slice(0, MAX_MENU_ITEMS);
    return new Promise<string | null>((resolve) => {
      this.pendingMenu = { id, resolve };
      void this.surface?.webview.postMessage({ type: "menu", menu: id, anchor, title, items: offered });
    });
  }

  /// Click a chip on the page as a person would. The journey and the eye pass drive this.
  clickChip(anchor: MenuAnchor): void {
    void this.surface?.webview.postMessage({ type: "clickChip", anchor });
  }

  private closeMenu(): void {
    const pending = this.pendingMenu;
    if (!pending) return;
    this.pendingMenu = null;
    void this.surface?.webview.postMessage({ type: "menuClose", menu: pending.id });
    pending.resolve(null);
  }

  private receiveMenuChoice(menu: string, choice: string | null): void {
    const pending = this.pendingMenu;
    if (!pending || pending.id !== menu) return;
    this.pendingMenu = null;
    pending.resolve(choice);
  }

  /// Open the newest declared change on the page in the diff editor, as a click on its button would. The
  /// journey and the eye pass drive this; a person clicks.
  openLatestDiff(): void {
    void this.surface?.webview.postMessage({ type: "openLatestDiff" });
  }

  /// Put the chosen mention text where the @ was typed, or (null) just hand focus back on cancel.
  insertComposerText(text: string | null): void {
    void this.surface?.webview.postMessage({ type: "insertText", text });
  }

  async measurePerformance(framesPerSecond = 3_000, durationMs = 5_000): Promise<WebviewPerformance> {
    if (framesPerSecond <= 0 || durationMs <= 0) {
      throw new Error("Webview measurement rate and duration must be positive");
    }
    let retryable: RetryableMeasurementError | null = null;
    for (let attempt = 0; attempt < MEASUREMENT_ATTEMPTS; attempt += 1) {
      try {
        await this.show(true);
        const surface = this.surface;
        if (!surface) throw new Error("the Runtrol conversation panel did not open");
        return await this.measurePerformanceOnce(surface, framesPerSecond, durationMs);
      } catch (error) {
        if (!(error instanceof RetryableMeasurementError)) throw error;
        retryable = error;
      }
    }
    throw retryable ?? new Error("the Runtrol Webview measurement did not run");
  }

  private async measurePerformanceOnce(
    surface: ConversationSurface,
    framesPerSecond: number,
    durationMs: number,
  ): Promise<WebviewPerformance> {
    const webview = surface.webview;
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const waiter = measurementWaiter();
    const droppedBefore = this.droppedFrames;
    this.measurements.set(id, waiter);
    try {
      if (!await webview.postMessage({ type: "measureStart", id })) {
        throw new Error("the Runtrol Webview closed before measurement started");
      }
      await withinMeasurementStage(
        waiter.ready,
        "the Runtrol Webview did not acknowledge measurement startup",
      );
      if (this.surface !== surface || !surface.visible || !this.visibleReady) {
        throw new RetryableMeasurementError("the Runtrol Webview changed before measurement load");
      }
      const total = Math.ceil(framesPerSecond * durationMs / 1_000);
      const started = performance.now();
      let produced = 0;
      while (produced < total) {
        const elapsed = performance.now() - started;
        const expected = Math.min(total, Math.floor(elapsed * framesPerSecond / 1_000));
        while (produced < expected) {
          this.frame(performanceFrame(produced));
          produced += 1;
        }
        await delay(10);
      }
      await this.drainPosts();
      if (!await webview.postMessage({
        type: "measureEnd",
        id,
        producedFrames: produced,
        droppedFrames: this.droppedFrames - droppedBefore,
      })) {
        throw new Error("the Runtrol Webview closed before measurement finished");
      }
      return await withinMeasurementStage(
        waiter.result,
        "the Runtrol Webview did not return measurement results",
      );
    } finally {
      this.measurements.delete(id);
    }
  }

  private async waitForVisibleWebview(surface: ConversationSurface): Promise<void> {
    const deadline = Date.now() + VISIBLE_READY_TIMEOUT_MS;
    const reloadAt = Date.now() + VISIBLE_READY_RELOAD_MS;
    let nextProbeAt = 0;
    let reloaded = false;
    while (this.surface === surface && Date.now() < deadline) {
      if (surface.visible && this.visibleReady) return;
      if (!reloaded && surface.visible && Date.now() >= reloadAt) {
        reloaded = true;
        this.visibleReady = false;
        surface.webview.html = this.html(surface.webview);
      }
      if (Date.now() >= nextProbeAt) {
        nextProbeAt = Date.now() + 250;
        void surface.webview.postMessage({ type: "readyProbe" });
      }
      await delay(25);
    }
    if (this.surface !== surface) {
      throw new Error("the Runtrol Webview closed before becoming ready");
    }
    throw new RetryableMeasurementError(
      `the visible Runtrol Webview was not ready within ${VISIBLE_READY_TIMEOUT_MS} ms `
      + `(place ${surface.place}, visible ${surface.visible}, ready ${this.visibleReady})`,
    );
  }

  private schedulePosts(): void {
    if (this.posting) {
      return;
    }
    this.posting = delay(16)
      .then(() => this.flushPosts())
      .finally(() => {
        this.posting = null;
        if (this.pendingFrames.length > 0) {
          this.schedulePosts();
        }
      });
  }

  private async flushPosts(): Promise<void> {
    while (this.surface && this.pendingFrames.length > 0) {
      const batch = this.pendingFrames.splice(0, POST_BATCH);
      const gap = this.postGap;
      this.postGap = false;
      const delivered = await this.surface.webview.postMessage({ type: "frames", batch, gap });
      if (!delivered) {
        this.pendingFrames = [];
        return;
      }
    }
  }

  private async drainPosts(): Promise<void> {
    while (this.posting || this.pendingFrames.length > 0) {
      await this.posting;
    }
  }

  private receiveMeasurement(value: unknown): boolean {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      return false;
    }
    const message = value as Record<string, unknown>;
    const id = typeof message.id === "string" ? message.id : "";
    const waiter = this.measurements.get(id);
    if (!waiter) {
      return false;
    }
    if (message.type === "measurementReady") {
      waiter.readyResolve();
      return true;
    }
    if (message.type === "performanceMeasurement") {
      const metrics = performanceMetrics(message.metrics);
      if (metrics) {
        waiter.resultResolve(metrics);
      } else {
        waiter.reject(
          new Error(`the Runtrol Webview returned malformed performance metrics: ${JSON.stringify(message.metrics)}`),
        );
      }
      return true;
    }
    return false;
  }

  private receiveRenderedGeneration(value: unknown): boolean {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
      return false;
    }
    const message = value as Record<string, unknown>;
    if (
      message.type !== "selectionRendered"
      || typeof message.generation !== "number"
      || !Number.isSafeInteger(message.generation)
    ) {
      return false;
    }
    this.renderedGeneration = Math.max(this.renderedGeneration, message.generation);
    for (const waiter of this.renderWaiters) {
      if (waiter.generation <= this.renderedGeneration) {
        this.renderWaiters.delete(waiter);
        waiter.resolve();
      }
    }
    return true;
  }

  private rejectMeasurements(error: Error): void {
    for (const waiter of this.measurements.values()) {
      waiter.reject(error);
    }
    this.measurements.clear();
  }

  private rejectRenderWaiters(error: Error): void {
    for (const waiter of this.renderWaiters) {
      waiter.reject(error);
    }
    this.renderWaiters.clear();
  }

  dispose(): void {
    this.closeMenu();
    this.dropSurfaceGuards();
    this.surface?.dispose();
    this.surface = null;
    this.visibleReady = false;
    this.pendingFrames = [];
    this.rejectMeasurements(new Error("the Runtrol conversation panel was disposed"));
    this.rejectRenderWaiters(new Error("the Runtrol conversation panel was disposed"));
  }

  private panelTitle(session: SessionLine | null): string {
    if (session) return `Runtrol: ${this.titleOf(session)}`;
    return this.draft ? "Runtrol: New chat" : "Runtrol Chat";
  }

  private html(webview: vscode.Webview): string {
    const script = webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, "dist", "webview.js"));
    const style = webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, "dist", "webview.css"));
    const nonce = nonceValue();
    return `<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';">
  <link rel="stylesheet" href="${style}">
  <title>Runtrol chat</title>
</head>
<body class="no-chat">
  <div id="status" role="status"></div>
  <main id="conversation" aria-live="polite"></main>
  <form id="composer">
    <!--
      The coding service's own commands, offered when the message starts with a slash. A listbox rather than
      decoration: the reader arrows through it and presses Enter, and a screen reader announces it as a choice.
    -->
    <ul id="commands" class="commands" role="listbox" aria-label="Commands this coding service offers" hidden></ul>
    <!-- Messages typed while the agent worked, sent one per turn boundary. This page's memory only. -->
    <ul id="queued" class="queued" aria-label="Messages waiting for the turn to end" hidden></ul>
    <!--
      One card, the shape every chat composer now has: where the conversation runs across the top, the
      message in the middle, and the controls along the bottom with send at the right edge.
    -->
    <div class="composer-card">
      <div class="composer-context">
        <button id="project-chip" class="chip chip-button" type="button" title="Project"></button>
        <span id="branch-chip" class="chip" hidden></span>
        <button id="service-chip" class="chip chip-button" type="button" title="Coding service"></button>
      </div>
      <ul id="attachments" class="attachments" aria-label="Images attached to the next message" hidden></ul>
      <!-- The popover a chip opens: the choices for that chip, answered where they were asked. -->
      <ul id="chip-menu" class="commands chip-menu" role="listbox" aria-label="Choices" hidden></ul>
      <textarea id="prompt" rows="1" aria-label="Message" placeholder="Message" disabled></textarea>
      <div class="composer-bar">
        <button id="attach" class="bar-button" type="button" aria-label="Add an image" title="Add an image" disabled>
          <span aria-hidden="true">+</span>
        </button>
        <button id="mode-chip" class="chip chip-button" type="button" title="Access mode" hidden></button>
        <span class="composer-spacer"></span>
        <button id="model-chip" class="chip chip-button" type="button" title="Model" hidden></button>
        <button id="effort-chip" class="chip chip-button" type="button" title="Reasoning effort" hidden></button>
        <button id="send" type="submit" aria-label="Send" title="Send" disabled hidden>
          <span aria-hidden="true">&#8593;</span>
        </button>
        <button id="interrupt" type="button" aria-label="Stop" title="Stop" disabled hidden>
          <span aria-hidden="true">&#9632;</span>
        </button>
      </div>
    </div>
    <div class="composer-foot">
      <span id="usage-chip" class="chip" hidden></span>
      <span id="send-hint" class="send-hint" hidden></span>
    </div>
  </form>
  <script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}

/// Bring a surface to the front and wait until VS Code agrees it is there: a tab has to be the active editor
/// of its group and the active tab of the window; a view only has to be visible.
function focusSurface(surface: ConversationSurface): Promise<void> {
  const focused = (): boolean => (
    surface.place === "tab"
      ? surface.visible && surface.active && conversationTabIsActive()
      : surface.visible
  );
  if (focused()) return Promise.resolve();
  return new Promise((resolve, reject) => {
    let settled = false;
    const listeners: vscode.Disposable[] = [];
    const settle = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      for (const listener of listeners) listener.dispose();
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    };
    const check = () => {
      if (focused()) settle();
    };
    const timeout = setTimeout(
      () => settle(new Error("VS Code did not activate the Runtrol conversation within 2000 ms")),
      2_000,
    );
    listeners.push(
      surface.onDidChangeVisibility(check),
      vscode.window.tabGroups.onDidChangeTabs(check),
      vscode.window.tabGroups.onDidChangeTabGroups(check),
      surface.onDidDispose(() => settle(new Error("the Runtrol conversation closed before activation"))),
    );
    surface.reveal(false);
    check();
  });
}

function conversationTabIsActive(): boolean {
  return vscode.window.tabGroups.all.some((group) => group.tabs.some((tab) => {
    if (!tab.isActive || !(tab.input instanceof vscode.TabInputWebview)) return false;
    return tab.input.viewType === "runtrol.conversation"
      || tab.input.viewType === "mainThreadWebview-runtrol.conversation";
  }));
}

function measurementWaiter(): MeasurementWaiter {
  let readyResolve: () => void = () => {};
  let rejectReady: (error: Error) => void = () => {};
  let resultResolve: (value: WebviewPerformance) => void = () => {};
  let rejectResult: (error: Error) => void = () => {};
  const ready = new Promise<void>((resolve, reject) => {
    readyResolve = resolve;
    rejectReady = reject;
  });
  const result = new Promise<WebviewPerformance>((resolve, reject) => {
    resultResolve = resolve;
    rejectResult = reject;
  });
  void result.catch(() => undefined);
  return {
    ready,
    readyResolve,
    result,
    resultResolve,
    reject: (error) => {
      rejectReady(error);
      rejectResult(error);
    },
  };
}

function withinMeasurementStage<T>(work: Promise<T>, message: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(
        () => reject(new RetryableMeasurementError(
          `${message} within ${MEASUREMENT_STAGE_TIMEOUT_MS} ms`,
        )),
        MEASUREMENT_STAGE_TIMEOUT_MS,
      );
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}

function performanceFrame(index: number): unknown {
  return {
    body: {
      event: "agentMessageChunk",
      content: { text: `frame ${index}\n` },
      delta: true,
      message_id: "load-stream",
    },
  };
}

function performanceMetrics(value: unknown): WebviewPerformance | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const metrics = value as Record<string, unknown>;
  const names: Array<keyof WebviewPerformance> = [
    "baselineFrameP95Ms",
    "frameP95Ms",
    "frameOverrunP95Ms",
    "inputP95Ms",
    "scrollP95Ms",
    "maxPendingFrames",
    "producedFrames",
    "droppedFrames",
    "visibleCharacters",
    "visibleItems",
  ];
  if (!names.every((name) => typeof metrics[name] === "number" && Number.isFinite(metrics[name]))) {
    return null;
  }
  return metrics as WebviewPerformance;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let index = 0; index < 32; index += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
