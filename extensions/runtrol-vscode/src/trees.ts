import * as vscode from "vscode";

import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { sessionStateLabel } from "./runtimeProjection";
import { orderedSessions } from "./sessionNavigation";
import { sessionContext, uniqueSessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";

export class SessionItem extends vscode.TreeItem {
  constructor(
    readonly session: SessionLine,
    selected: boolean,
    sessions: readonly SessionLine[],
    providers: readonly ProviderLine[],
  ) {
    const title = uniqueSessionTitle(session, sessions, providers);
    super(title, vscode.TreeItemCollapsibleState.None);
    const state = sessionStateLabel(session);
    this.description = `${state}${session.looksStuck ? " · needs attention" : ""}`;
    this.tooltip = [
      title,
      sessionContext(session, providers),
      session.workspace,
      `State: ${state}`,
    ].join("\n");
    this.contextValue = "runtrol.session";
    this.command = {
      command: "runtrol.selectSession",
      title: "Focus Session",
      arguments: [this],
    };
    this.iconPath = new vscode.ThemeIcon(
      session.looksStuck ? "warning" : session.hot ? "circle-filled" : "circle-outline",
      session.looksStuck
        ? new vscode.ThemeColor("problemsWarningIcon.foreground")
        : selected
          ? new vscode.ThemeColor("charts.green")
          : undefined,
    );
  }
}

export class SessionsTree implements vscode.TreeDataProvider<SessionItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private readonly subscription: vscode.Disposable;

  constructor(private readonly state: RuntimeState) {
    this.subscription = state.onDidChange(() => this.changedEmitter.fire());
  }

  getTreeItem(element: SessionItem): vscode.TreeItem {
    return element;
  }

  getChildren(): SessionItem[] {
    const selected = this.state.selected?.sessionId ?? null;
    return orderedSessions(this.state.sessions, selected)
      .map((session) => new SessionItem(
        session,
        session.sessionId === selected,
        this.state.sessions,
        this.state.providers,
      ));
  }

  dispose(): void {
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }
}

class ProviderItem extends vscode.TreeItem {
  constructor(provider: ProviderLine) {
    super(provider.displayName, vscode.TreeItemCollapsibleState.None);
    const usable = provider.installation.state === "usable";
    this.description = usable ? "Ready" : "Unavailable";
    this.tooltip = provider.installation.why ?? `${provider.displayName} is ready`;
    this.iconPath = new vscode.ThemeIcon(
      usable ? "terminal" : "circle-slash",
      usable ? undefined : new vscode.ThemeColor("disabledForeground"),
    );
  }
}

export class ProvidersTree implements vscode.TreeDataProvider<ProviderItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private readonly subscription: vscode.Disposable;

  constructor(private readonly state: RuntimeState) {
    this.subscription = state.onDidChange(() => this.changedEmitter.fire());
  }

  getTreeItem(element: ProviderItem): vscode.TreeItem {
    return element;
  }

  getChildren(): ProviderItem[] {
    return this.state.providers.map((provider) => new ProviderItem(provider));
  }

  dispose(): void {
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }
}
