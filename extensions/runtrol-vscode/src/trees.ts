import path from "node:path";

import * as vscode from "vscode";

import type { ProviderLine, SessionLine } from "./protocol";
import { RuntimeState } from "./state";

export class SessionItem extends vscode.TreeItem {
  constructor(readonly session: SessionLine, selected: boolean) {
    const folder = path.basename(session.workspace) || session.workspace;
    super(folder, vscode.TreeItemCollapsibleState.None);
    this.description = `${session.provider}  ${session.doing}`;
    this.tooltip = new vscode.MarkdownString(
      `**${folder}**\n\n${session.workspace}\n\nProvider: ${session.provider}\n\nState: ${session.doing}`,
    );
    this.contextValue = "runtrol.session";
    this.command = {
      command: "runtrol.selectSession",
      title: "Focus Session",
      arguments: [this],
    };
    this.iconPath = new vscode.ThemeIcon(
      session.looks_stuck ? "warning" : session.hot ? "circle-filled" : "circle-outline",
      session.looks_stuck
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
    const selected = this.state.selected?.session;
    return this.state.sessions
      .slice()
      .sort((left, right) => Number(right.hot) - Number(left.hot))
      .map((session) => new SessionItem(session, session.session === selected));
  }

  dispose(): void {
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }
}

class ProviderItem extends vscode.TreeItem {
  constructor(provider: ProviderLine) {
    super(provider.display_name, vscode.TreeItemCollapsibleState.None);
    this.description = provider.usable ? "Ready" : "Unavailable";
    this.tooltip = provider.why_not ?? `${provider.display_name} is ready`;
    this.iconPath = new vscode.ThemeIcon(
      provider.usable ? "terminal" : "circle-slash",
      provider.usable ? undefined : new vscode.ThemeColor("disabledForeground"),
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

