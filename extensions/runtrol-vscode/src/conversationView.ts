import * as vscode from "vscode";

import type { SessionLine } from "./runtimeTypes";
import { providerDisplayName, sessionTitle } from "./sessionDisplay";
import { webviewReadyKind } from "./webviewReady";

type ViewAction =
  | { type: "prompt"; text: string }
  | { type: "startChat" }
  | { type: "answerApproval"; approval: string; option: number; subjectDigest: number[] }
  | { type: "openWorkspace" }
  | { type: "interrupt" }
  | { type: "close" };

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
  private panel: vscode.WebviewPanel | null = null;
  private selected: SessionLine | null = null;
  private generation = 0;
  private pendingFrames: FrameEnvelope[] = [];
  private posting: Promise<void> | null = null;
  private postGap = false;
  private droppedFrames = 0;
  private readonly measurements = new Map<string, MeasurementWaiter>();
  private readonly renderWaiters = new Set<RenderWaiter>();
  private renderedGeneration = 0;
  private visibleReady = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly action: (message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string = sessionTitle,
    private readonly visibility: (visible: boolean) => void = () => {},
    private readonly providerOf: (session: SessionLine) => string = (session) => providerDisplayName(session.providerId),
  ) {}

  async show(preserveFocus = false): Promise<void> {
    if (this.panel) {
      const panel = this.panel;
      if (preserveFocus) {
        if (!panel.visible) {
          panel.reveal(panel.viewColumn ?? vscode.ViewColumn.Active, true);
        }
      } else {
        await focusPanel(panel);
      }
      await this.waitForVisibleWebview(panel);
      return;
    }
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
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, "resources", "symbol.svg");
    this.panel = panel;
    panel.webview.onDidReceiveMessage((message: unknown) => {
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
          this.reset(this.selected);
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
      this.action(message);
    });
    panel.onDidChangeViewState(({ webviewPanel }) => {
      if (webviewPanel.visible || this.panel !== panel) {
        return;
      }
      this.visibleReady = false;
      this.visibility(false);
      this.rejectMeasurements(new RetryableMeasurementError(
        "the Runtrol Webview became hidden during measurement",
      ));
    });
    panel.onDidDispose(() => {
      if (this.panel === panel) {
        this.panel = null;
        this.visibleReady = false;
        this.visibility(false);
        this.pendingFrames = [];
        this.rejectMeasurements(new Error("the Runtrol Webview closed during measurement"));
        this.rejectRenderWaiters(new Error("the Runtrol Webview closed before painting the selected session"));
      }
    });
    panel.webview.html = this.html(panel.webview);
    if (preserveFocus) {
      panel.reveal(panel.viewColumn ?? vscode.ViewColumn.Active, true);
    } else {
      await focusPanel(panel);
    }
    await this.waitForVisibleWebview(panel);
  }

  reset(session: SessionLine | null): number {
    this.selected = session;
    if (this.panel) {
      this.panel.title = this.panelTitle(session);
    }
    this.generation += 1;
    this.pendingFrames = [];
    this.postGap = false;
    void this.panel?.webview.postMessage({
      type: "reset",
      session,
      title: session ? this.titleOf(session) : null,
      provider: session ? this.providerOf(session) : null,
      generation: this.generation,
    });
    return this.generation;
  }

  updateSession(session: SessionLine): void {
    this.selected = session;
    if (this.panel) {
      this.panel.title = this.panelTitle(session);
    }
    void this.panel?.webview.postMessage({
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
    if (!this.panel?.visible || !this.visibleReady) {
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
    void this.panel?.webview.postMessage({ type: "status", message, kind });
  }

  async measurePerformance(framesPerSecond = 3_000, durationMs = 5_000): Promise<WebviewPerformance> {
    if (framesPerSecond <= 0 || durationMs <= 0) {
      throw new Error("Webview measurement rate and duration must be positive");
    }
    let retryable: RetryableMeasurementError | null = null;
    for (let attempt = 0; attempt < MEASUREMENT_ATTEMPTS; attempt += 1) {
      try {
        await this.show(true);
        const panel = this.panel;
        if (!panel) throw new Error("the Runtrol conversation panel did not open");
        return await this.measurePerformanceOnce(panel, framesPerSecond, durationMs);
      } catch (error) {
        if (!(error instanceof RetryableMeasurementError)) throw error;
        retryable = error;
      }
    }
    throw retryable ?? new Error("the Runtrol Webview measurement did not run");
  }

  private async measurePerformanceOnce(
    panel: vscode.WebviewPanel,
    framesPerSecond: number,
    durationMs: number,
  ): Promise<WebviewPerformance> {
    const webview = panel.webview;
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
      if (this.panel !== panel || !panel.visible || !this.visibleReady) {
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

  private async waitForVisibleWebview(panel: vscode.WebviewPanel): Promise<void> {
    const deadline = Date.now() + VISIBLE_READY_TIMEOUT_MS;
    const reloadAt = Date.now() + VISIBLE_READY_RELOAD_MS;
    let nextProbeAt = 0;
    let reloaded = false;
    while (this.panel === panel && Date.now() < deadline) {
      if (panel.visible && this.visibleReady) return;
      if (!reloaded && panel.visible && Date.now() >= reloadAt) {
        reloaded = true;
        this.visibleReady = false;
        panel.webview.html = this.html(panel.webview);
      }
      if (Date.now() >= nextProbeAt) {
        nextProbeAt = Date.now() + 250;
        void panel.webview.postMessage({ type: "readyProbe" });
      }
      await delay(25);
    }
    if (this.panel !== panel) {
      throw new Error("the Runtrol Webview closed before becoming ready");
    }
    throw new RetryableMeasurementError(
      `the visible Runtrol Webview was not ready within ${VISIBLE_READY_TIMEOUT_MS} ms`,
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
    while (this.panel && this.pendingFrames.length > 0) {
      const batch = this.pendingFrames.splice(0, POST_BATCH);
      const gap = this.postGap;
      this.postGap = false;
      const delivered = await this.panel.webview.postMessage({ type: "frames", batch, gap });
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
    this.panel?.dispose();
    this.panel = null;
    this.visibleReady = false;
    this.pendingFrames = [];
    this.rejectMeasurements(new Error("the Runtrol conversation panel was disposed"));
    this.rejectRenderWaiters(new Error("the Runtrol conversation panel was disposed"));
  }

  private panelTitle(session: SessionLine | null): string {
    return session ? `Runtrol: ${this.titleOf(session)}` : "Runtrol Chat";
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
  <header id="chat-header">
    <div class="service-avatar" id="service-avatar" aria-hidden="true">R</div>
    <div class="chat-heading">
      <strong id="session-title">Choose a chat</strong>
      <div class="chat-meta">
        <strong id="service-name">Runtrol</strong>
        <span class="context-separator" aria-hidden="true">·</span>
        <span id="session-path"></span>
      </div>
      <div class="session-facts" aria-label="Session configuration">
        <span id="session-model" hidden></span>
        <span id="session-effort" hidden></span>
        <span id="session-mode" hidden></span>
      </div>
    </div>
    <div class="header-actions">
      <span id="session-state"></span>
      <button id="open-workspace" type="button" title="Open workspace" hidden>Open workspace</button>
      <button id="close" type="button" title="Close chat" hidden>Close</button>
    </div>
  </header>
  <section id="usage-summary" aria-label="Usage and limits" hidden>
    <div class="usage-card" id="context-usage" hidden>
      <span class="usage-label">Context</span>
      <strong id="context-value">Unavailable</strong>
      <div class="usage-meter" aria-hidden="true"><span id="context-meter"></span></div>
    </div>
    <div class="usage-card" id="session-cost" hidden>
      <span class="usage-label">Session cost</span>
      <strong id="cost-value">Unavailable</strong>
    </div>
    <div class="usage-card" id="primary-limit" hidden>
      <span class="usage-label" id="primary-label">Short limit</span>
      <strong id="primary-value">Unavailable</strong>
      <span class="usage-reset" id="primary-reset"></span>
      <div class="usage-meter" aria-hidden="true"><span id="primary-meter"></span></div>
    </div>
    <div class="usage-card" id="secondary-limit" hidden>
      <span class="usage-label" id="secondary-label">Long limit</span>
      <strong id="secondary-value">Unavailable</strong>
      <span class="usage-reset" id="secondary-reset"></span>
      <div class="usage-meter" aria-hidden="true"><span id="secondary-meter"></span></div>
    </div>
  </section>
  <div id="status" role="status"></div>
  <main id="conversation" aria-live="polite"></main>
  <form id="composer">
    <textarea id="prompt" rows="3" aria-label="Message" placeholder="Write a message" disabled></textarea>
    <div class="actions">
      <span class="send-hint" id="send-hint" hidden>Ctrl+Enter to send</span>
      <button id="interrupt" type="button" disabled hidden>Interrupt</button>
      <button id="send" type="submit" disabled hidden>Send</button>
    </div>
  </form>
  <script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}

function focusPanel(panel: vscode.WebviewPanel): Promise<void> {
  if (panel.visible && panel.active && conversationTabIsActive()) return Promise.resolve();
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
      if (panel.visible && panel.active && conversationTabIsActive()) settle();
    };
    const timeout = setTimeout(
      () => settle(new Error("VS Code did not activate the Runtrol conversation tab within 2000 ms")),
      2_000,
    );
    listeners.push(
      panel.onDidChangeViewState(check),
      vscode.window.tabGroups.onDidChangeTabs(check),
      vscode.window.tabGroups.onDidChangeTabGroups(check),
      panel.onDidDispose(() => settle(new Error("the Runtrol conversation tab closed before activation"))),
    );
    panel.reveal(panel.viewColumn ?? vscode.ViewColumn.Active, false);
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

function isViewAction(value: unknown): value is ViewAction {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const type = (value as { type?: unknown }).type;
  if (type === "prompt") {
    return typeof (value as { text?: unknown }).text === "string";
  }
  if (type === "answerApproval") {
    const candidate = value as {
      approval?: unknown;
      option?: unknown;
      subjectDigest?: unknown;
    };
    return typeof candidate.approval === "string"
      && typeof candidate.option === "number"
      && Array.isArray(candidate.subjectDigest)
      && candidate.subjectDigest.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255);
  }
  return type === "startChat"
    || type === "openWorkspace"
    || type === "interrupt"
    || type === "close";
}

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let index = 0; index < 32; index += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
