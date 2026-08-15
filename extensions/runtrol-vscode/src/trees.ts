import * as vscode from "vscode";

import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { sessionStateLabel } from "./runtimeProjection";
import { chatServices, type ChatService } from "./sessionNavigation";
import { sessionContext, uniqueChatTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";

const SELECTION_PRESENTATION_DELAY_MS = 150;

export class ServiceItem extends vscode.TreeItem {
  readonly startProviderId: string | null;

  constructor(readonly service: ChatService) {
    super(
      service.displayName,
      service.sessions.length > 0
        ? vscode.TreeItemCollapsibleState.Expanded
        : vscode.TreeItemCollapsibleState.None,
    );
    const count = service.sessions.length;
    const usable = service.provider?.installation.state === "usable";
    const chatAvailable = usable || count > 0;
    this.id = `runtrol.service.${encodeURIComponent(service.providerId)}`;
    this.startProviderId = usable ? service.providerId : null;
    this.description = count > 0
      ? `${count} ${count === 1 ? "chat" : "chats"}`
      : usable
        ? "New chat"
        : "Unavailable";
    this.tooltip = [
      service.displayName,
      count > 0 ? `${count} provider-owned ${count === 1 ? "chat" : "chats"}` : "No chats yet",
      service.provider?.installation.why ?? (usable ? "Ready to start a chat" : "Provider is not currently listed"),
    ].join("\n");
    this.contextValue = usable ? "runtrol.service.ready" : "runtrol.service.unavailable";
    this.iconPath = new vscode.ThemeIcon(
      chatAvailable ? "comment-discussion" : "circle-slash",
      service.selected
        ? new vscode.ThemeColor("charts.green")
        : chatAvailable
          ? undefined
          : new vscode.ThemeColor("disabledForeground"),
    );
    this.accessibilityInformation = {
      label: `${service.displayName}, ${this.description}`,
    };
  }
}

export class SessionItem extends vscode.TreeItem {
  constructor(
    readonly session: SessionLine,
    selected: boolean,
    sessions: readonly SessionLine[],
    providers: readonly ProviderLine[],
  ) {
    const title = uniqueChatTitle(session, sessions);
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
      title: "Open Chat",
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

export type ChatTreeItem = ServiceItem | SessionItem;

export class SessionsTree implements vscode.TreeDataProvider<ChatTreeItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private readonly subscription: vscode.Disposable;
  private selectionRefresh: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly state: RuntimeState) {
    this.subscription = state.onDidChange((change) => {
      if (change === "rows") {
        this.cancelSelectionRefresh();
        this.changedEmitter.fire();
        return;
      }
      this.cancelSelectionRefresh();
      this.selectionRefresh = setTimeout(() => {
        this.selectionRefresh = undefined;
        this.changedEmitter.fire();
      }, SELECTION_PRESENTATION_DELAY_MS);
    });
  }

  getTreeItem(element: ChatTreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: ChatTreeItem): ChatTreeItem[] {
    const selected = this.state.selected?.sessionId ?? null;
    if (element instanceof ServiceItem) {
      return element.service.sessions
        .map((session) => new SessionItem(
          session,
          session.sessionId === selected,
          element.service.sessions,
          this.state.providers,
        ));
    }
    if (element) {
      return [];
    }
    return chatServices(this.state.sessions, this.state.providers, selected)
      .map((service) => new ServiceItem(service));
  }

  dispose(): void {
    this.cancelSelectionRefresh();
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }

  private cancelSelectionRefresh(): void {
    if (this.selectionRefresh) {
      clearTimeout(this.selectionRefresh);
      this.selectionRefresh = undefined;
    }
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
    this.subscription = state.onDidChange((change) => {
      if (change === "rows") {
        this.changedEmitter.fire();
      }
    });
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
