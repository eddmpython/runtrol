import * as vscode from "vscode";

import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderLine,
  SessionLine,
} from "./runtimeTypes";
import { sessionStateLabel } from "./runtimeProjection";
import { chatServices, type ChatService } from "./sessionNavigation";
import { sessionContext, uniqueChatTitle, workspaceName } from "./sessionDisplay";
import { RuntimeState } from "./state";

export class ServiceItem extends vscode.TreeItem {
  readonly startProviderId: string | null;

  constructor(
    readonly service: ChatService,
    readonly catalogue: NativeChatCatalogue | null,
  ) {
    const count = service.sessions.length + service.nativeChats.length;
    const usable = service.provider?.installation.state === "usable";
    super(
      service.displayName,
      count > 0 || usable
        ? vscode.TreeItemCollapsibleState.Expanded
        : vscode.TreeItemCollapsibleState.None,
    );
    const chatAvailable = usable || count > 0;
    this.id = `runtrol.service.${encodeURIComponent(service.providerId)}`;
    this.startProviderId = usable ? service.providerId : null;
    this.description = serviceDescription(count, Boolean(usable));
    this.tooltip = [
      service.displayName,
      count > 0 ? `${count} existing ${count === 1 ? "chat" : "chats"}` : "No chats yet",
      service.provider?.installation.why ?? (usable ? "Ready to start a chat" : "Provider is not currently listed"),
      catalogue?.warning ?? undefined,
    ].filter((line): line is string => Boolean(line)).join("\n");
    this.contextValue = usable ? "runtrol.service.ready" : "runtrol.service.unavailable";
    this.iconPath = new vscode.ThemeIcon(
      chatAvailable ? "comment-discussion" : "circle-slash",
      chatAvailable ? undefined : new vscode.ThemeColor("disabledForeground"),
    );
    this.accessibilityInformation = {
      label: `${service.displayName}, ${this.description}`,
    };
  }
}

function serviceDescription(
  count: number,
  usable: boolean,
): string {
  if (count > 0) return `${count} ${count === 1 ? "chat" : "chats"}`;
  if (!usable) return "Unavailable";
  return "Ready";
}

export class NewChatItem extends vscode.TreeItem {
  constructor(readonly startProviderId: string, displayName: string) {
    super("New chat", vscode.TreeItemCollapsibleState.None);
    this.description = `with ${displayName}`;
    this.contextValue = "runtrol.newChat";
    this.command = {
      command: "runtrol.startServiceChat",
      title: "New Chat",
      arguments: [this],
    };
    this.iconPath = new vscode.ThemeIcon("add");
    this.accessibilityInformation = { label: `Start a new chat with ${displayName}` };
  }
}

export class NativeSessionItem extends vscode.TreeItem {
  constructor(readonly native: NativeChatLine) {
    const title = native.title?.trim() || workspaceName(native.cwd);
    super(title, vscode.TreeItemCollapsibleState.None);
    const resumable = native.resume === "available" && Boolean(native.adoptionToken);
    this.description = resumable ? "Continue chat" : "Cannot continue";
    this.tooltip = [
      title,
      native.cwd,
      native.updatedAt ? `Updated: ${native.updatedAt}` : undefined,
      resumable ? "Open this existing chat" : "This coding service cannot resume this chat",
    ].filter((line): line is string => Boolean(line)).join("\n");
    this.contextValue = resumable ? "runtrol.nativeSession" : "runtrol.nativeSession.unavailable";
    if (resumable) {
      this.command = {
        command: "runtrol.selectSession",
        title: "Open Existing Chat",
        arguments: [this],
      };
    }
    this.iconPath = new vscode.ThemeIcon(resumable ? "history" : "circle-slash");
  }
}

export class SessionItem extends vscode.TreeItem {
  constructor(
    readonly session: SessionLine,
    sessions: readonly SessionLine[],
    providers: readonly ProviderLine[],
  ) {
    const title = uniqueChatTitle(session, sessions);
    super(title, vscode.TreeItemCollapsibleState.None);
    const state = sessionStateLabel(session);
    this.description = state;
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
        : undefined,
    );
  }
}

export type ChatTreeItem = ServiceItem | NewChatItem | SessionItem | NativeSessionItem;

export class SessionsTree implements vscode.TreeDataProvider<ChatTreeItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<ChatTreeItem | undefined>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private readonly subscription: vscode.Disposable;
  private serviceItems: ServiceItem[] | undefined;
  private readonly serviceSessions = new Map<string, ChatTreeItem[]>();

  constructor(
    private readonly state: RuntimeState,
    private readonly discoverNative: (providerId: string) => void = () => undefined,
  ) {
    this.subscription = state.onDidChange((change) => {
      if (change === "rows") {
        this.clearItems();
        this.changedEmitter.fire(undefined);
      }
    });
  }

  getTreeItem(element: ChatTreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: ChatTreeItem): ChatTreeItem[] {
    this.ensureItems();
    if (element instanceof ServiceItem) {
      if (element.startProviderId && !element.catalogue) {
        this.discoverNative(element.startProviderId);
      }
      return this.serviceSessions.get(element.service.providerId) ?? [];
    }
    if (element) {
      return [];
    }
    for (const service of this.serviceItems ?? []) {
      if (service.startProviderId && !service.catalogue) {
        this.discoverNative(service.startProviderId);
      }
    }
    return this.serviceItems ?? [];
  }

  dispose(): void {
    this.clearItems();
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }

  private ensureItems(): void {
    if (this.serviceItems) {
      return;
    }
    const selected = this.state.selected?.sessionId ?? null;
    const services = chatServices(
      this.state.sessions,
      this.state.providers,
      selected,
      this.state.nativeChats,
    );
    this.serviceItems = services.map((service) => new ServiceItem(
      service,
      this.state.nativeCatalogue(service.providerId),
    ));
    for (const service of services) {
      const items: ChatTreeItem[] = [
        ...(service.provider?.installation.state === "usable"
          ? [new NewChatItem(service.providerId, service.displayName)]
          : []),
        ...service.sessions.map((session) => new SessionItem(
          session,
          service.sessions,
          this.state.providers,
        )),
        ...service.nativeChats.map((native) => new NativeSessionItem(native)),
      ];
      this.serviceSessions.set(service.providerId, items);
    }
  }

  private clearItems(): void {
    this.serviceItems = undefined;
    this.serviceSessions.clear();
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
