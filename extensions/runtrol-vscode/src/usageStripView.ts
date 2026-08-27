/// The usage strip as a VS Code webview view, pinned under the conversation list in the Runtrol container.
///
/// The host owns every fact on the page: it derives the rows the same way the tree once did, renders them
/// with `usageStripHtml`, and hands the page nothing else. The page sends back exactly one kind of message,
/// the action a panel offered, and the host runs it through the controller like any other button.

import { randomBytes } from "node:crypto";

import * as vscode from "vscode";

import { conversationIcon } from "./conversationIcon";
import type { RuntimeState } from "./state";
import { usageRows } from "./usageDisplay";
import { usageChips, usageStripHtml, type UsageChip } from "./usageStrip";

export const USAGE_VIEW_ID = "runtrol.usage";

export type UsageStripActions = {
  signIn(providerId: string): Promise<void>;
  fix(providerId: string): Promise<void>;
};

export class UsageStripView implements vscode.WebviewViewProvider, vscode.Disposable {
  private view: vscode.WebviewView | null = null;
  private readonly subscription: { dispose(): void };
  private lastRendered = "";

  constructor(
    private readonly state: RuntimeState,
    private readonly extensionUri: vscode.Uri,
    private readonly actions: UsageStripActions,
    private readonly report: (error: unknown) => void,
  ) {
    this.subscription = state.onDidChange((change) => {
      if (change === "selection") return;
      this.render();
    });
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "resources", "provider-icons")],
    };
    view.onDidDispose(() => {
      if (this.view === view) this.view = null;
    });
    view.webview.onDidReceiveMessage((message: unknown) => {
      const action = readAction(message);
      if (!action) return;
      const run = action.action === "signIn"
        ? this.actions.signIn(action.providerId)
        : this.actions.fix(action.providerId);
      run.catch(this.report);
    });
    this.lastRendered = "";
    this.render();
  }

  /// Draw the current rows. Skipped when nothing the page shows has changed, so a selection tick or an
  /// unrelated row change does not blank and redraw the strip under the pointer.
  private render(): void {
    const view = this.view;
    if (!view) return;
    const chips = usageChips(usageRows(this.state.usage, this.state.providers, Date.now()));
    const key = JSON.stringify(chips);
    if (key === this.lastRendered) return;
    this.lastRendered = key;
    view.webview.html = usageStripHtml(chips, {
      cspSource: view.webview.cspSource,
      nonce: randomBytes(16).toString("base64url"),
      iconUris: iconUris(chips, view.webview, this.extensionUri),
    });
  }

  dispose(): void {
    this.subscription.dispose();
    this.view = null;
  }
}

function iconUris(
  chips: readonly UsageChip[],
  webview: vscode.Webview,
  extensionUri: vscode.Uri,
): Map<string, string> {
  const uris = new Map<string, string>();
  for (const chip of chips) {
    if (uris.has(chip.icon)) continue;
    uris.set(chip.icon, webview.asWebviewUri(conversationIcon(extensionUri, chip.icon)).toString());
  }
  return uris;
}

function readAction(message: unknown): { action: "signIn" | "fix"; providerId: string } | null {
  if (!message || typeof message !== "object") return null;
  const { type, action, providerId } = message as Record<string, unknown>;
  if (type !== "action" || typeof providerId !== "string") return null;
  if (action !== "signIn" && action !== "fix") return null;
  return { action, providerId };
}
