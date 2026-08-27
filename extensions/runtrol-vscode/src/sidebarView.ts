/// The sidebar as a VS Code webview view: the one view in the Runtrol container.
///
/// One view, deliberately. VS Code draws a collapsible section header for every view in a container as soon as
/// there are two, and it moves the title actions into those headers; the page then shows "Runtrol" twice and
/// the add buttons leave the title bar. With one view the container's title bar keeps the two actions that start
/// things and the page below draws everything else (`docs/vscodeSurface.md`).
///
/// The host owns every fact on the page: it derives the rows the same way every other surface does (from
/// `RuntimeState`), renders them with `sidebarHtml`, and hands the page nothing else. The page sends back what
/// was pressed, and every press becomes an ordinary command invocation with a `sidebarTargets` object.

import { randomBytes } from "node:crypto";

import * as vscode from "vscode";

import { conversationIcon } from "./conversationIcon";
import { canDelete } from "./conversationDeletion";
import { loose, projects, type Conversation, type ProjectGroup } from "./conversationList";
import type { ProjectRecord } from "./projects";
import { projectColorId } from "./projectColor";
import { awaitsVerification, isUsable } from "./providerHealth";
import type { ProviderCapabilities } from "./runtimeTypes";
import { ConversationItem, ProjectItem, ServiceChoiceItem } from "./sidebarTargets";
import {
  formatMemory,
  rowKeys,
  sidebarHtml,
  type SidebarConversationRow,
  type SidebarMenuItem,
  type SidebarModel,
  type SidebarNotice,
  type SidebarProjectRow,
  type SidebarServiceChoice,
} from "./sidebarPage";
import type { RuntimeState } from "./state";
import { usageChips, type UsageChip } from "./usageStrip";
import { usageRows } from "./usageDisplay";

export const SIDEBAR_VIEW_ID = "runtrol.sidebar";

const COLLAPSED_KEY = "runtrol.sidebar.collapsedProjects";

export type ProjectsPort = {
  all(): readonly ProjectRecord[];
  onDidChange(listener: () => void): { dispose(): void };
  reorder(keys: readonly string[]): Promise<void>;
};

export type AgentToolsPort = {
  enabled(workspace: string): boolean;
  onDidChange(listener: () => void): { dispose(): void };
};

export type UsageActions = {
  signIn(providerId: string): Promise<void>;
  fix(providerId: string): Promise<void>;
};

type ServiceOffer = () => readonly { providerId: string; displayName: string; icon: string }[];

/// Everything rare enough to live behind the vertical dots rather than in the title bar.
const MENU: readonly SidebarMenuItem[] = [
  { command: "runtrol.openNextWaiting", label: "Open the next conversation waiting for you" },
  { command: "runtrol.switchSession", label: "Switch conversation..." },
  { command: "runtrol.refresh", label: "Look again" },
  { command: "runtrol.setUpServices", label: "Set up coding services" },
  { command: "runtrol.checkProviderUpdates", label: "Check for service updates" },
  { command: "runtrol.pairPhone", label: "Pair a phone" },
  { command: "runtrol.managePhones", label: "Manage phones" },
  { command: "runtrol.reviewIntegrations", label: "Review Runtime integrations" },
  { command: "runtrol.manageIntegrations", label: "Manage Runtime integrations" },
  { command: "runtrol.reviewRuntimeRequests", label: "Review Runtime requests" },
  { command: "runtrol.restartExtensionHost", label: "Restart the Extension Host" },
];

export class SidebarView implements vscode.WebviewViewProvider, vscode.Disposable {
  private view: vscode.WebviewView | null = null;
  private readonly subscriptions: { dispose(): void }[] = [];
  private lastRendered = "";
  private model: SidebarModel | null = null;
  private groups: readonly ProjectGroup[] = [];
  private choosingFor: string | null = null;
  private services: ServiceOffer | null = null;
  private updateNotice: string | null = null;
  private staleWindow: string | null = null;
  private collapsed: Set<string>;
  private usableProvider: boolean | null = null;
  private verifyingProvider: boolean | null = null;
  private reach: string | null = null;
  private pendingReveal: string | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly state: RuntimeState,
    private readonly projectRecords: ProjectsPort,
    private readonly agentTools: AgentToolsPort,
    private readonly usageActions: UsageActions,
    private readonly report: (error: unknown) => void,
  ) {
    this.collapsed = new Set(context.workspaceState.get<string[]>(COLLAPSED_KEY, []));
    this.subscriptions.push(
      state.onDidChange((change) => {
        if (change === "selection") return;
        this.render();
      }),
      projectRecords.onDidChange(() => this.render()),
      agentTools.onDidChange(() => this.render()),
      vscode.workspace.onDidChangeWorkspaceFolders(() => this.render()),
    );
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.context.extensionUri, "resources", "provider-icons")],
    };
    view.onDidDispose(() => {
      if (this.view === view) this.view = null;
    });
    view.onDidChangeVisibility(() => {
      if (view.visible) this.render();
    });
    view.webview.onDidReceiveMessage((message: unknown) => {
      this.receive(message).catch(this.report);
    });
    this.lastRendered = "";
    this.render();
  }

  /// The row keys in page order: what the eye tests read to prove no conversation is drawn twice.
  treeItemIds(): string[] {
    return this.model ? rowKeys(this.model) : [];
  }

  /// The services' own reasons the list is not everything, for the menu's explanation.
  listingReasons(): string | null {
    return this.state.incompleteDiscovery;
  }

  setUpdateNotice(notice: string | null): void {
    if (this.updateNotice === notice) return;
    this.updateNotice = notice;
    this.render();
  }

  setStaleWindow(notice: string | null): void {
    this.staleWindow = notice;
    this.render();
  }

  offerServices(services: ServiceOffer): void {
    this.services = services;
  }

  /// A service choice for a new conversation in `workspace`: offered in the page, above the list.
  chooseService(workspace: string): void {
    this.choosingFor = workspace;
    this.render();
  }

  clearServiceChoice(): void {
    if (this.choosingFor === null) return;
    this.choosingFor = null;
    this.render();
  }

  async revealConversation(key: string): Promise<void> {
    this.pendingReveal = key;
    this.flushReveal();
  }

  async revealSession(sessionId: string): Promise<void> {
    const row = this.state.conversations.find((candidate) => candidate.session?.sessionId === sessionId);
    if (row) await this.revealConversation(row.key);
  }

  dispose(): void {
    for (const subscription of this.subscriptions) subscription.dispose();
    this.view = null;
  }

  private async receive(message: unknown): Promise<void> {
    if (!message || typeof message !== "object") return;
    const { type } = message as Record<string, unknown>;
    if (type === "ready") {
      this.flushReveal();
      return;
    }
    if (type === "collapse") {
      const { key, collapsed } = message as { key?: unknown; collapsed?: unknown };
      if (typeof key !== "string") return;
      if (collapsed === true) this.collapsed.add(key);
      else this.collapsed.delete(key);
      await this.context.workspaceState.update(COLLAPSED_KEY, [...this.collapsed]);
      return;
    }
    if (type === "reorder") {
      const { keys } = message as { keys?: unknown };
      if (!Array.isArray(keys) || !keys.every((key) => typeof key === "string")) return;
      await this.projectRecords.reorder(keys.map((key: string) => decodeProjectKey(key)));
      return;
    }
    if (type === "action") {
      // The usage chips speak the strip's own message: sign in or fix, by provider.
      const { action, providerId } = message as { action?: unknown; providerId?: unknown };
      if (typeof providerId !== "string") return;
      if (action === "signIn") await this.usageActions.signIn(providerId);
      else if (action === "fix") await this.usageActions.fix(providerId);
      return;
    }
    if (type === "command") {
      const { command, target } = message as { command?: unknown; target?: unknown };
      if (typeof command !== "string" || !command.startsWith("runtrol.")) return;
      const item = this.targetOf(target);
      if (item === undefined) await vscode.commands.executeCommand(command);
      else await vscode.commands.executeCommand(command, item);
    }
  }

  private targetOf(target: unknown): ConversationItem | ProjectItem | ServiceChoiceItem | undefined {
    if (!target || typeof target !== "object") return undefined;
    const { kind, key, workspace } = target as { kind?: unknown; key?: unknown; workspace?: unknown };
    if (typeof key !== "string") return undefined;
    if (kind === "project") {
      const group = this.groups.find((candidate) => candidate.key === key);
      return group ? new ProjectItem(group, this.agentTools.enabled(group.workspace)) : undefined;
    }
    if (kind === "conversation") {
      const row = this.state.conversations.find((candidate) => candidate.key === key);
      return row ? new ConversationItem(row) : undefined;
    }
    if (kind === "service" && typeof workspace === "string") {
      return new ServiceChoiceItem(key, workspace);
    }
    return undefined;
  }

  private flushReveal(): void {
    const view = this.view;
    const key = this.pendingReveal;
    if (!view || !key) return;
    this.pendingReveal = null;
    void view.webview.postMessage({ type: "reveal", key });
  }

  private render(): void {
    this.updateContext();
    const view = this.view;
    if (!view) return;
    const model = this.buildModel();
    this.model = model;
    this.updateBadge(view, model);
    const key = JSON.stringify(model);
    if (key === this.lastRendered) return;
    this.lastRendered = key;
    const icons = new Map<string, string>();
    const iconUri = (declared: string): void => {
      if (icons.has(declared)) return;
      icons.set(declared, view.webview.asWebviewUri(conversationIcon(this.context.extensionUri, declared)).toString());
    };
    for (const project of model.projects) for (const row of project.rows) iconUri(row.icon);
    for (const row of model.loose) iconUri(row.icon);
    for (const chip of model.usage) iconUri(chip.icon);
    for (const service of model.serviceChoice?.services ?? []) iconUri(service.icon);
    view.webview.html = sidebarHtml(model, {
      cspSource: view.webview.cspSource,
      nonce: randomBytes(16).toString("base64url"),
      iconUris: icons,
    });
    this.flushReveal();
  }

  private buildModel(): SidebarModel {
    const rows = this.state.conversations;
    const openWorkspaces = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
    this.groups = projects(this.projectRecords.all(), rows, openWorkspaces);
    const projectRows: SidebarProjectRow[] = this.groups.map((group) => ({
      key: group.key,
      name: group.name,
      workspace: group.workspace,
      color: projectColorId(group.workspace),
      kind: group.kind,
      pinned: group.pinned,
      current: group.current,
      collapsed: this.collapsed.has(group.key),
      attention: group.attention,
      live: group.live,
      agentTools: this.agentTools.enabled(group.workspace),
      rows: pinnedFirst(group.rows).map((row) => this.conversationRow(row, projectColorId(group.workspace))),
    }));
    const looseRows = pinnedFirst(loose(rows)).map((row) => this.conversationRow(row, null));
    const usage: UsageChip[] = usageChips(usageRows(this.state.usage, this.state.providers, Date.now()));
    const usable = this.state.providers.some(isUsable);
    const firstRun = projectRows.length === 0 && looseRows.length === 0
      && this.state.coreReach === "reached" && usable;
    return {
      notices: this.notices(usable),
      projects: projectRows,
      loose: looseRows,
      usage,
      serviceChoice: this.serviceChoice(),
      firstRun,
      menu: this.state.incompleteDiscovery
        ? [{ command: "runtrol.explainListing", label: "Why is the list incomplete?" }, ...MENU]
        : MENU,
    };
  }

  private conversationRow(row: Conversation, color: string | null): SidebarConversationRow {
    const capabilities: ProviderCapabilities | null = this.state.providerCapabilities(row.providerId);
    const memoryBytes = this.state.memoryFor(row) ?? row.session?.memoryBytes ?? null;
    return {
      key: row.key,
      title: row.title,
      serviceName: row.serviceName,
      icon: row.serviceIcon,
      color,
      activity: row.activity,
      live: row.live,
      canOpen: row.canOpen,
      blocked: row.blocked,
      pinned: row.pinned,
      signIn: row.signInNeeded,
      canDelete: canDelete(row, capabilities),
      canArchive: row.native !== null && capabilities?.nativeSessionArchive?.availability === "available",
      memory: typeof memoryBytes === "number" ? formatMemory(memoryBytes) : null,
      tool: row.tool,
      workspace: row.workspace,
    };
  }

  private serviceChoice(): SidebarServiceChoice | null {
    if (this.choosingFor === null) return null;
    const services = this.services?.() ?? [];
    return services.length === 0 ? null : { workspace: this.choosingFor, services };
  }

  private notices(usable: boolean): SidebarNotice[] {
    const notices: SidebarNotice[] = [];
    if (this.staleWindow) {
      notices.push({ tone: "warn", text: this.staleWindow, command: null, label: null });
    }
    if (this.state.coreReach === "unreachable") {
      notices.push({
        tone: "error",
        text: "Cannot reach the Runtrol Core. Your conversations are still on this machine; this window is trying again.",
        command: "runtrol.refresh",
        label: "Try now",
      });
    } else if (this.state.coreReach === "connecting") {
      notices.push({ tone: "info", text: "Connecting to the Runtrol Core...", command: null, label: null });
    } else if (!usable && this.state.providers.some(awaitsVerification)) {
      notices.push({ tone: "info", text: "Checking the installed coding-agent CLI...", command: null, label: null });
    } else if (!usable) {
      notices.push({
        tone: "warn",
        text: "No coding-agent CLI was found on this machine. Runtrol supervises Claude Code, Codex and Grok; install one and sign in with its own command.",
        command: "runtrol.setUpServices",
        label: "Set up",
      });
    }
    if (this.updateNotice) {
      notices.push({ tone: "info", text: this.updateNotice, command: null, label: null });
    }
    const history = this.state.discoveryNotice;
    if (history) {
      notices.push({ tone: "info", text: history, command: "runtrol.explainListing", label: "Why" });
    }
    return notices;
  }

  /// The count on the activity bar icon, so a blocked agent is visible from a different view entirely.
  private updateBadge(view: vscode.WebviewView, model: SidebarModel): void {
    const waiting = [...model.projects.flatMap((project) => project.rows), ...model.loose]
      .filter((row) => row.activity === "needsYou").length;
    view.badge = waiting > 0
      ? { value: waiting, tooltip: waiting === 1 ? "1 conversation needs you" : `${waiting} conversations need you` }
      : undefined;
  }

  /// Context keys for the title-bar menus, which live outside the page.
  private updateContext(): void {
    const usable = this.state.providers.some(isUsable);
    const verifying = !usable && this.state.providers.some(awaitsVerification);
    const reach = this.state.coreReach;
    if (usable === this.usableProvider && verifying === this.verifyingProvider && reach === this.reach) {
      return;
    }
    this.usableProvider = usable;
    this.verifyingProvider = verifying;
    this.reach = reach;
    void vscode.commands.executeCommand("setContext", "runtrol.hasUsableProvider", usable);
    void vscode.commands.executeCommand("setContext", "runtrol.isVerifyingProvider", verifying);
    void vscode.commands.executeCommand("setContext", "runtrol.coreReach", reach);
    void vscode.commands.executeCommand("setContext", "runtrol.listingIncomplete", this.state.incompleteDiscovery !== null);
  }
}

function pinnedFirst(rows: readonly Conversation[]): Conversation[] {
  return [...rows.filter((row) => row.pinned), ...rows.filter((row) => !row.pinned)];
}

/// A page project key (`project:<encoded record key>`) back to the record key the store is addressed by.
function decodeProjectKey(pageKey: string): string {
  return pageKey.startsWith("project:") ? decodeURIComponent(pageKey.slice("project:".length)) : pageKey;
}
