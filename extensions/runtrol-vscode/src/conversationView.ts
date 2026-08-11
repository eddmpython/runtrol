import * as vscode from "vscode";

import type { SessionLine } from "./protocol";

type ViewAction =
  | { type: "prompt"; text: string }
  | { type: "answerApproval"; approval: string; option: number; subjectDigest: number[] }
  | { type: "openWorkspace" }
  | { type: "interrupt" }
  | { type: "close" };

export class ConversationView implements vscode.WebviewViewProvider {
  static readonly viewType = "runtrol.conversation";
  private view: vscode.WebviewView | null = null;
  private selected: SessionLine | null = null;

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
      if (!isViewAction(message)) {
        return;
      }
      this.action(message);
    });
    view.onDidDispose(() => {
      if (this.view === view) {
        this.view = null;
      }
    });
    this.reset(this.selected);
  }

  reset(session: SessionLine | null): void {
    this.selected = session;
    void this.view?.webview.postMessage({ type: "reset", session });
  }

  frame(payload: unknown): void {
    void this.view?.webview.postMessage({ type: "frame", payload });
  }

  status(message: string, kind: "info" | "warning" | "error" = "info"): void {
    void this.view?.webview.postMessage({ type: "status", message, kind });
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
