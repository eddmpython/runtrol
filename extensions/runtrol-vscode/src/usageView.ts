import * as vscode from "vscode";

import { isBroken } from "./providerHealth";
import type { ProviderLine, ProviderUsageGauge, ProviderUsageList } from "./runtimeTypes";
import { setupRows, usageRows, usageRowsEqual, type SetupRow, type UsageRow } from "./usageDisplay";
import { usageViewAction, type UsageViewSnapshot } from "./usageViewMessage";
import { webviewNonce } from "./webviewNonce";

/// Every installed CLI's operational state and usage, fixed at the bottom of the primary sidebar.
///
/// This is a Webview because a native tree row cannot draw a progress bar. It receives only bounded structural
/// usage snapshots, never session events or conversation content. Hidden views retain no browser state.
export class UsageView implements vscode.WebviewViewProvider, vscode.Disposable {
  private gauges: readonly ProviderUsageGauge[] = [];
  private rows: UsageRow[] = [];
  private setup: SetupRow[] = [];
  /// Whether the set-up list is showing. Held by the host so the title bar's plus can open it, and so a
  /// redraw does not close it under the reader's hand.
  private setupOpen = false;
  private error: string | null = null;
  private view: vscode.WebviewView | null = null;
  private viewSubscriptions: vscode.Disposable[] = [];
  private fetching = false;
  private behind = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly ports: {
      usage: () => Promise<ProviderUsageList>;
      providers: () => readonly ProviderLine[];
      now: () => number;
      fix: (provider: ProviderLine) => Promise<void>;
      signIn: (provider: ProviderLine) => Promise<void>;
      setUp: (provider: ProviderLine) => Promise<void>;
      dispatch: (action: () => Promise<void>) => void;
    },
  ) {}

  get visible(): boolean {
    return this.view?.visible ?? false;
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.clearViewSubscriptions();
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.extensionUri, "dist"),
        vscode.Uri.joinPath(this.extensionUri, "resources", "provider-icons"),
      ],
    };
    view.webview.html = this.html(view.webview);
    this.viewSubscriptions.push(
      view.webview.onDidReceiveMessage((message) => this.onMessage(message)),
      view.onDidChangeVisibility(() => {
        if (view.visible) void this.refreshQuietly();
      }),
      view.onDidDispose(() => {
        if (this.view === view) this.view = null;
        this.clearViewSubscriptions();
      }),
    );
    this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()), true);
    if (view.visible) void this.refreshQuietly();
  }

  /// A pushed usage snapshot: drawn at once, with no request in between.
  usageChanged(gauges: readonly ProviderUsageGauge[]): void {
    this.gauges = gauges;
    this.setError(null);
    this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()));
  }

  sessionsChanged(): void {
    // Provider availability is already in memory. Draw that state immediately instead of waiting behind the usage
    // request, while retaining the last explicitly aged gauge until a fresh snapshot arrives.
    this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()));
    if (this.visible) void this.refreshQuietly();
  }

  async refresh(): Promise<void> {
    if (this.fetching) {
      this.behind = true;
      return;
    }
    this.fetching = true;
    try {
      const usage = await this.ports.usage();
      this.gauges = usage.providers;
      this.setError(null);
      this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()));
    } finally {
      this.fetching = false;
      if (this.behind) {
        this.behind = false;
        void this.refreshQuietly();
      }
    }
  }

  dispose(): void {
    this.view = null;
    this.clearViewSubscriptions();
  }

  private async refreshQuietly(): Promise<void> {
    try {
      await this.refresh();
    } catch (error) {
      // Previous reports remain visible, but the failure must not look like a successful current snapshot, and
      // it must say what went wrong: an unexplained red line in the panel is a dead end for the reader.
      const why = error instanceof Error ? error.message : String(error);
      this.setError(`Usage refresh failed: ${why}. Showing the last report.`);
    }
  }

  /// Open the set-up list, from the section title's plus.
  ///
  /// It opens in the panel itself rather than in a picker at the top of the window: the reader pressed a control
  /// in the sidebar, and sending their eye to the title bar to answer is the interruption this product exists to
  /// avoid.
  openSetup(): void {
    this.setupOpen = true;
    this.postSnapshot();
    void this.view?.show?.(true);
  }

  private publish(next: UsageRow[], force = false): void {
    const setup = setupRows(this.ports.providers());
    const changed = !usageRowsEqual(this.rows, next) || JSON.stringify(setup) !== JSON.stringify(this.setup);
    this.rows = next;
    this.setup = setup;
    if (changed || force) this.postSnapshot();
  }

  private setError(error: string | null): void {
    if (error === this.error) return;
    this.error = error;
    this.postSnapshot();
  }

  private postSnapshot(): void {
    const message: UsageViewSnapshot = {
      type: "snapshot",
      rows: this.rows,
      setup: this.setupOpen ? this.setup : [],
      error: this.error,
    };
    void this.view?.webview.postMessage(message);
  }

  private onMessage(message: unknown): void {
    const action = usageViewAction(message);
    if (!action) return;
    if (action.type === "ready") {
      this.postSnapshot();
      return;
    }
    const provider = this.ports.providers().find((candidate) => candidate.providerId === action.providerId);
    if (!provider) return;
    if (action.type === "setUp") {
      this.ports.dispatch(() => this.ports.setUp(provider));
      return;
    }
    if (action.type === "signIn") {
      if (provider.account?.status !== "signedOut") return;
      this.ports.dispatch(() => this.ports.signIn(provider));
      return;
    }
    if (!isBroken(provider)) return;
    this.ports.dispatch(() => this.ports.fix(provider));
  }

  private clearViewSubscriptions(): void {
    const subscriptions = this.viewSubscriptions;
    this.viewSubscriptions = [];
    for (const subscription of subscriptions) subscription.dispose();
  }

  private html(webview: vscode.Webview): string {
    const script = webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, "dist", "usageView.js"));
    const style = webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, "dist", "usageView.css"));
    // The service glyphs are the same theme-aware SVGs the conversation surfaces use, addressed as a folder so
    // the Webview builds one `<img>` per row from the manifest's declared name. Provider-neutral: no icon table
    // reaches this file, only a base the row name is appended to.
    const iconBase = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, "resources", "provider-icons"),
    );
    const nonce = webviewNonce();
    return `<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; img-src ${webview.cspSource}; script-src 'nonce-${nonce}';">
  <link rel="stylesheet" href="${style}">
  <title>Agent usage</title>
</head>
<body>
  <main id="usage" aria-live="polite" data-icon-base="${iconBase}"></main>
  <p id="empty" class="empty" hidden>No connected CLI.</p>
  <p id="error" class="error" role="status" hidden></p>
  <section id="setup" class="setup" hidden></section>
  <script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }
}
