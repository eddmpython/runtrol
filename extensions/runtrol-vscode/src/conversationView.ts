import * as vscode from "vscode";

import type { SessionLine } from "./protocol";

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

const MAX_PENDING_POSTS = 4_096;
const POST_BATCH = 512;

export class ConversationView implements vscode.WebviewViewProvider {
  static readonly viewType = "runtrol.conversation";
  private view: vscode.WebviewView | null = null;
  private selected: SessionLine | null = null;
  private generation = 0;
  private pendingFrames: FrameEnvelope[] = [];
  private posting: Promise<void> | null = null;
  private postGap = false;
  private readonly measurements = new Map<string, MeasurementWaiter>();

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly action: (message: ViewAction) => void,
  ) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "dist")],
    };
    view.webview.html = this.html(view.webview);
    view.webview.onDidReceiveMessage((message: unknown) => {
      if (this.receiveMeasurement(message)) {
        return;
      }
      if (!isViewAction(message)) {
        return;
      }
      this.action(message);
    });
    view.onDidDispose(() => {
      if (this.view === view) {
        this.view = null;
        this.pendingFrames = [];
        this.rejectMeasurements(new Error("the Runtrol Webview closed during measurement"));
      }
    });
    this.reset(this.selected);
  }

  reset(session: SessionLine | null): void {
    this.selected = session;
    this.generation += 1;
    this.pendingFrames = [];
    this.postGap = false;
    void this.view?.webview.postMessage({ type: "reset", session, generation: this.generation });
  }

  frame(payload: unknown): void {
    if (!this.view) {
      return;
    }
    if (this.pendingFrames.length >= MAX_PENDING_POSTS) {
      this.pendingFrames.splice(0, this.pendingFrames.length - MAX_PENDING_POSTS + 1);
      this.postGap = true;
    }
    this.pendingFrames.push({ generation: this.generation, payload });
    this.schedulePosts();
  }

  status(message: string, kind: "info" | "warning" | "error" = "info"): void {
    void this.view?.webview.postMessage({ type: "status", message, kind });
  }

  async measurePerformance(framesPerSecond = 3_000, durationMs = 5_000): Promise<WebviewPerformance> {
    if (!this.view) {
      throw new Error("open the Runtrol view before measuring its Webview");
    }
    if (framesPerSecond <= 0 || durationMs <= 0) {
      throw new Error("Webview measurement rate and duration must be positive");
    }
    const webview = this.view.webview;
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const waiter = measurementWaiter();
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
      if (!await webview.postMessage({ type: "measureEnd", id, producedFrames: produced })) {
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
    while (this.view && this.pendingFrames.length > 0) {
      const batch = this.pendingFrames.splice(0, POST_BATCH);
      const gap = this.postGap;
      this.postGap = false;
      const delivered = await this.view.webview.postMessage({ type: "frames", batch, gap });
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

  private rejectMeasurements(error: Error): void {
    for (const waiter of this.measurements.values()) {
      waiter.reject(error);
    }
    this.measurements.clear();
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
  <title>Runtrol active session</title>
</head>
<body>
  <header>
    <div>
      <strong id="session-title">No active session</strong>
      <div id="session-path"></div>
    </div>
    <button id="open-workspace" type="button" title="Open workspace">Open</button>
  </header>
  <div id="status" role="status"></div>
  <main id="conversation" aria-live="polite"></main>
  <form id="composer">
    <textarea id="prompt" rows="3" placeholder="Select a session to send a prompt" disabled></textarea>
    <div class="actions">
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

function nonceValue(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let value = "";
  for (let index = 0; index < 32; index += 1) {
    value += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return value;
}
