import * as vscode from "vscode";

import type { SessionLine } from "./runtimeTypes";
import { sessionTitle } from "./sessionDisplay";

type ViewAction =
  | { type: "prompt"; text: string }
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

type WebviewReadyWaiter = {
  promise: Promise<void>;
  resolve(): void;
};

const MAX_PENDING_POSTS = 4_096;
const POST_BATCH = MAX_PENDING_POSTS;

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
  private webviewReady: WebviewReadyWaiter | null = null;
  private visibleReady = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly action: (message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string = sessionTitle,
    private readonly visibility: (visible: boolean) => void = () => {},
  ) {}

  show(preserveFocus = false): Promise<void> {
    if (this.panel) {
      this.panel.reveal(this.panel.viewColumn ?? vscode.ViewColumn.Active, preserveFocus);
      return this.webviewReady?.promise ?? Promise.resolve();
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
    const ready = webviewReadyWaiter();
    this.webviewReady = ready;
    panel.webview.onDidReceiveMessage((message: unknown) => {
      if (isWebviewReady(message)) {
        this.visibleReady = true;
        this.webviewReady?.resolve();
        this.reset(this.selected);
        this.visibility(true);
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
      this.webviewReady?.resolve();
      this.webviewReady = webviewReadyWaiter();
      this.visibility(false);
    });
    panel.onDidDispose(() => {
      if (this.panel === panel) {
        this.panel = null;
        this.visibleReady = false;
        this.visibility(false);
        this.webviewReady?.resolve();
        this.webviewReady = null;
        this.pendingFrames = [];
        ready.resolve();
        this.rejectMeasurements(new Error("the Runtrol Webview closed during measurement"));
        this.rejectRenderWaiters(new Error("the Runtrol Webview closed before painting the selected session"));
      }
    });
    panel.webview.html = this.html(panel.webview);
    return ready.promise;
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
      generation: this.generation,
    });
    return this.generation;
  }

  updateSession(session: SessionLine): void {
    this.selected = session;
    if (this.panel) {
      this.panel.title = this.panelTitle(session);
    }
    void this.panel?.webview.postMessage({ type: "session", session, title: this.titleOf(session) });
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
    await this.show(true);
    const panel = this.panel;
    if (!panel) throw new Error("the Runtrol conversation panel did not open");
    if (framesPerSecond <= 0 || durationMs <= 0) {
      throw new Error("Webview measurement rate and duration must be positive");
    }
    const webview = panel.webview;
    const ready = this.webviewReady;
    if (!ready) {
      throw new Error("open the Runtrol view before measuring its Webview");
    }
    await ready.promise;
    if (this.panel?.webview !== webview) {
      throw new Error("the Runtrol Webview changed before measurement started");
    }
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const waiter = measurementWaiter();
    const droppedBefore = this.droppedFrames;
    this.measurements.set(id, waiter);
    const timeout = setTimeout(
      () => waiter.reject(new Error("the Runtrol Webview measurement exceeded 30 seconds")),
      30_000,
    );
    try {
      if (!await webview.postMessage({ type: "measureStart", id })) {
        throw new Error("the Runtrol Webview closed before measurement started");
      }
      await waiter.ready;
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
      return await waiter.result;
    } finally {
      clearTimeout(timeout);
      this.measurements.delete(id);
    }
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
    this.webviewReady?.resolve();
    this.webviewReady = null;
    this.pendingFrames = [];
    this.rejectMeasurements(new Error("the Runtrol conversation panel was disposed"));
    this.rejectRenderWaiters(new Error("the Runtrol conversation panel was disposed"));
  }

  private panelTitle(session: SessionLine | null): string {
    return session ? `Runtrol: ${this.titleOf(session)}` : "Runtrol Session";
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
  <title>Runtrol session</title>
</head>
<body>
  <header>
    <div class="session-heading">
      <strong id="session-title">No active session</strong>
      <div id="session-path"></div>
    </div>
    <div class="header-actions">
      <span id="session-state"></span>
      <button id="open-workspace" type="button" title="Open workspace">Open workspace</button>
    </div>
  </header>
  <div id="status" role="status"></div>
  <main id="conversation" aria-live="polite"></main>
  <form id="composer">
    <textarea id="prompt" rows="2" aria-label="Prompt" placeholder="Select a session to send a prompt" disabled></textarea>
    <div class="actions">
      <span class="send-hint">Ctrl+Enter to send</span>
      <button id="interrupt" type="button" disabled>Interrupt</button>
      <button id="close" type="button" disabled>Close</button>
      <button id="send" type="submit" disabled>Send</button>
    </div>
  </form>
  <script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}

function webviewReadyWaiter(): WebviewReadyWaiter {
  let settled = false;
  let resolvePromise: () => void = () => {};
  const promise = new Promise<void>((resolve) => {
    resolvePromise = resolve;
  });
  return {
    promise,
    resolve: () => {
      if (settled) return;
      settled = true;
      resolvePromise();
    },
  };
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
  return type === "openWorkspace" || type === "interrupt" || type === "close";
}

function isWebviewReady(value: unknown): boolean {
  return value !== null
    && typeof value === "object"
    && !Array.isArray(value)
    && (value as Record<string, unknown>).type === "webviewReady";
}

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let index = 0; index < 32; index += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
