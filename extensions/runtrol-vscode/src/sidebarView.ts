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

import { accentedConversationIcon, conversationIcon } from "./conversationIcon";
import { canDelete } from "./conversationDeletion";
import { loose, projects, stopping, type Conversation, type ProjectGroup } from "./conversationList";
import type { ProjectRecord } from "./projects";
import { projectAccentColor } from "./projectColor";
import { readGitBranch } from "./gitBranch";
import type { GitChanges } from "./gitChanges";
import { awaitsVerification, isUsable } from "./providerHealth";
import type { ProviderCapabilities } from "./runtimeTypes";
import { ConversationItem, ProjectItem, ServiceChoiceItem } from "./sidebarTargets";
import {
  formatMemory,
  rowKeys,
  sidebarBody,
  sidebarHtml,
  type SidebarConversationRow,
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
const EXPANDED_KEY = "runtrol.sidebar.expandedProjects";

/// How many conversations a project shows before the rest wait behind "Show all".
///
/// Five, because a machine with several projects is the case that matters: one project with forty
/// conversations pushed every other project off the screen, and the panel is meant to show the machine
/// (operator, 2026-08-28).
const ROWS_PER_PROJECT = 5;

/// How often a project's branch is read again.
const BRANCH_READ_EVERY_MS = 5_000;

export type ProjectsPort = {
  all(): readonly ProjectRecord[];
  onDidChange(listener: () => void): { dispose(): void };
  reorder(keys: readonly string[]): Promise<void>;
};

/// The uncommitted and unpushed work per project folder, measured by `GitChangesWatch` on its own triggers.
export type GitChangesPort = {
  get(workspace: string): GitChanges | null | undefined;
  ensure(workspace: string): void;
  keep(workspaces: readonly string[]): void;
  onDidChange(listener: () => void): { dispose(): void };
};

export type UsageActions = {
  signIn(providerId: string): Promise<void>;
  signOut(providerId: string): Promise<void>;
  fix(providerId: string): Promise<void>;
  /// Update the service's CLI to the release the Update button names.
  update(providerId: string): Promise<void>;
};

export type ConversationTabsPort = {
  isOpen(conversationKey: string): boolean;
  onDidChange(listener: () => void): { dispose(): void };
};

/// What the Core's private help line says per service (`ProviderHelpCache`).
export type ProviderHelpPort = {
  signOutFor(providerId: string): string | null;
  onDidChange(listener: () => void): { dispose(): void };
};

/// What the Core last said about each service's release (`ProviderUpdateWatch`).
export type ProviderReleasePort = {
  installedFor(providerId: string): string | null;
  updateTargetFor(providerId: string): string | null;
  onDidChange(listener: () => void): { dispose(): void };
};

type ServiceOffer = () => readonly { providerId: string; displayName: string; icon: string }[];

/// Everything rare enough to live behind the vertical dots rather than in the title bar.
export class SidebarView implements vscode.WebviewViewProvider, vscode.Disposable {
  private view: vscode.WebviewView | null = null;
  private readonly subscriptions: { dispose(): void }[] = [];
  private lastRendered = "";
  /// The nonce of the document currently in the view, or null while no document has been written. A repaint
  /// changes only the body, so the head that carries the policy and the scripts has to stay the one written
  /// here.
  private documentNonce: string | null = null;
  private model: SidebarModel | null = null;
  private groups: readonly ProjectGroup[] = [];
  private choosingFor: string | null = null;
  private services: ServiceOffer | null = null;
  private staleWindow: string | null = null;
  private collapsed: Set<string>;
  private expanded: Set<string>;
  /// The branch each project's folder is on, as last read. A render is synchronous and a repository read is
  /// not, so the chip shows what the previous read found and the next read corrects it.
  private branches = new Map<string, string | null>();
  /// When the branches were last read. The panel redraws on every session update and a branch moves when a
  /// person switches one, so the read is neither per redraw nor once: it is at most this often.
  private branchesReadAt = 0;
  private usableProvider: boolean | null = null;
  private verifyingProvider: boolean | null = null;
  private reach: string | null = null;
  private pendingReveal: string | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly state: RuntimeState,
    private readonly projectRecords: ProjectsPort,
    private readonly changes: GitChangesPort,
    private readonly releases: ProviderReleasePort,
    private readonly help: ProviderHelpPort,
    private readonly usageActions: UsageActions,
    private readonly tabs: ConversationTabsPort,
    private readonly report: (error: unknown) => void,
  ) {
    this.collapsed = new Set(context.workspaceState.get<string[]>(COLLAPSED_KEY, []));
    this.expanded = new Set(context.workspaceState.get<string[]>(EXPANDED_KEY, []));
    this.subscriptions.push(
      state.onDidChange((change) => {
        if (change === "selection") return;
        this.render();
      }),
      projectRecords.onDidChange(() => this.render()),
      changes.onDidChange(() => this.render()),
      releases.onDidChange(() => this.render()),
      help.onDidChange(() => this.render()),
      tabs.onDidChange(() => this.render()),
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
    this.documentNonce = null;
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
    if (type === "expand") {
      const { key } = message as { key?: unknown };
      if (typeof key !== "string") return;
      this.expanded.add(key);
      await this.context.workspaceState.update(EXPANDED_KEY, [...this.expanded]);
      this.render();
      return;
    }
    if (type === "dismissChoice") {
      // The person walked away from the service question: a click elsewhere, Escape, or focus leaving.
      this.clearServiceChoice();
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
      else if (action === "signOut") await this.usageActions.signOut(providerId);
      else if (action === "fix") await this.usageActions.fix(providerId);
      else if (action === "update") await this.usageActions.update(providerId);
      return;
    }
    if (type === "command") {
      const { command, target } = message as { command?: unknown; target?: unknown };
      if (typeof command !== "string" || !command.startsWith("runtrol.")) return;
      const item = this.targetOf(target);
      if (target !== undefined && item === undefined) {
        throw new Error("This sidebar item is no longer available. Refresh the list and try again.");
      }
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
      return group ? new ProjectItem(group) : undefined;
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
    // Keep the packaged container title as the one heading. Assigning a view title makes VS Code synthesize a
    // colon, while the derived release manifest already names this container `Runtrol 0.1.42` exactly.
    if (view.title !== undefined) view.title = undefined;
    const key = JSON.stringify(model);
    if (key === this.lastRendered) return;
    this.lastRendered = key;
    const icons = new Map<string, string>();
    const accentIcons = new Map<string, string>();
    const iconUri = (declared: string): void => {
      if (icons.has(declared)) return;
      icons.set(declared, view.webview.asWebviewUri(conversationIcon(this.context.extensionUri, declared)).toString());
    };
    for (const project of model.projects) for (const row of project.rows) iconUri(row.icon);
    for (const row of model.loose) iconUri(row.icon);
    for (const row of [...model.projects.flatMap((project) => project.rows), ...model.loose]) {
      const key = `${row.icon}\0${row.accent}`;
      if (!accentIcons.has(key)) {
        accentIcons.set(
          key,
          accentedConversationIcon(this.context.extensionUri, row.icon, row.accent).toString(true),
        );
      }
    }
    for (const chip of model.usage) iconUri(chip.icon);
    for (const service of model.serviceChoice?.services ?? []) iconUri(service.icon);
    const assets = {
      cspSource: view.webview.cspSource,
      nonce: this.documentNonce ?? randomBytes(16).toString("base64url"),
      iconUris: icons,
      accentIconUris: accentIcons,
    };
    if (this.documentNonce === null) {
      this.documentNonce = assets.nonce;
      view.webview.html = sidebarHtml(model, assets);
    } else {
      // Only the content changes. The document, its scripts and everything they hold (the open detail panel,
      // the focused row, the scroll position) stay exactly where the person left them.
      void view.webview.postMessage({ type: "paint", body: sidebarBody(model, assets) });
    }
    this.flushReveal();
  }

  private buildModel(): SidebarModel {
    const rows = this.state.conversations;
    const openWorkspaces = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
    this.groups = projects(this.projectRecords.all(), rows, openWorkspaces);
    void this.readBranches(this.groups);
    this.changes.keep(this.groups.map((group) => group.workspace));
    for (const group of this.groups) this.changes.ensure(group.workspace);
    const projectRows: SidebarProjectRow[] = this.groups.map((group) => ({
      key: group.key,
      name: group.name,
      workspace: group.workspace,
      kind: group.kind,
      pinned: group.pinned,
      current: group.current,
      collapsed: this.collapsed.has(group.key),
      attention: group.attention,
      live: group.live,
      branch: this.branches.get(group.key) ?? null,
      changes: this.changes.get(group.workspace) ?? null,
      ...this.rowsOf(group),
    }));
    const looseRows = pinnedFirst(loose(rows)).map((row) => this.conversationRow(row, projectAccentColor(null)));
    // Which services publish a sign-in line, so the account panel can offer it whatever the account's state
    // is. A person switching accounts, or checking one that looks healthy, had no way there at all.
    const signInAble = new Set(this.state.providers
      .filter((provider) => provider.help?.signIn)
      .map((provider) => provider.providerId));
    const signOutAble = new Set(this.state.providers
      .filter((provider) => this.help.signOutFor(provider.providerId) !== null)
      .map((provider) => provider.providerId));
    // The CLI release beside each service's name: Runtime's probe of the binary, or the Core's inspection when
    // the probe said nothing. And the release the Update button goes to, when the Core confirmed one.
    const releases = new Map(this.state.providers.map((provider) => [provider.providerId, {
      version: provider.installation.version ?? this.releases.installedFor(provider.providerId),
      updateTo: this.releases.updateTargetFor(provider.providerId),
    }] as const));
    const usage: UsageChip[] = usageChips(
      usageRows(this.state.usage, this.state.providers, Date.now()),
      signInAble,
      releases,
      signOutAble,
    );
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
      version: this.version(),
    };
  }

  /// This build's version, from the installed extension's manifest. The checked-in manifest carries the
  /// derived-version placeholder, so an unpackaged dev build has no number to show and the line stays away.
  private version(): string {
    const declared = this.context.extension.packageJSON as { version?: unknown };
    return typeof declared.version === "string" && declared.version !== "0.0.0" ? declared.version : "";
  }

  /// Read every project's branch once per listing, and redraw if any of them moved.
  ///
  /// Off the render path on purpose: the read is bounded file reads, not a subprocess, but it is still I/O and
  /// the panel is redrawn on every session update. What the chip shows is the last answer.
  private async readBranches(groups: readonly ProjectGroup[]): Promise<void> {
    const now = Date.now();
    const missing = groups.some((group) => !this.branches.has(group.key));
    if (!missing && now - this.branchesReadAt < BRANCH_READ_EVERY_MS) return;
    this.branchesReadAt = now;
    let changed = false;
    for (const group of groups) {
      const branch = await readGitBranch(group.workspace).catch(() => null);
      if (this.branches.get(group.key) === branch) continue;
      this.branches.set(group.key, branch);
      changed = true;
    }
    for (const key of [...this.branches.keys()]) {
      if (!groups.some((group) => group.key === key)) this.branches.delete(key);
    }
    if (changed) this.render();
  }

  /// The rows this project shows now, and how many are waiting behind "Show all".
  private rowsOf(group: ProjectGroup): { rows: SidebarConversationRow[]; hidden: number } {
    const ordered = pinnedFirst(group.rows);
    const shown = this.expanded.has(group.key) ? ordered : ordered.slice(0, ROWS_PER_PROJECT);
    return {
      rows: shown.map((row) => this.conversationRow(row, projectAccentColor(group.workspace))),
      hidden: ordered.length - shown.length,
    };
  }

  private conversationRow(row: Conversation, accent: string): SidebarConversationRow {
    const capabilities: ProviderCapabilities | null = this.state.providerCapabilities(row.providerId);
    const memoryBytes = this.state.memoryFor(row) ?? row.session?.memoryBytes ?? null;
    return {
      key: row.key,
      title: row.title,
      spawnedBy: row.spawnedBy,
      serviceName: row.serviceName,
      icon: row.serviceIcon,
      accent,
      open: this.tabs.isOpen(row.key) || (row.hostedKey !== null && this.tabs.isOpen(row.hostedKey)),
      activity: row.activity,
      live: row.live,
      canStop: row.canStop,
      canOpen: row.canOpen,
      canFocus: row.canFocus,
      stopping: stopping(row),
      dialogue: row.hostedTerminal && row.hostedTerminal.origin !== "observedMirror"
        && row.hostedTerminal.processState === "running" ? row.hostedTerminal.dialogueEnabled ?? false : undefined,
      blocked: row.blocked,
      pinned: row.pinned,
      signIn: row.signInNeeded,
      canDelete: canDelete(row, capabilities),
      canArchive: row.presence.kind !== "unconfirmed"
        && row.native !== null
        && capabilities?.nativeSessionArchive?.availability === "available",
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
        text: "No coding-agent CLI was found on this machine. Runtrol supervises Claude Code and Codex; install one and sign in with its own command.",
        command: "runtrol.setUpServices",
        label: "Set up",
      });
    }
    // The services' own "history is partial" sentence is not pushed here. It sat above every list taking a
    // line to say something no one acts on, and the same answer is one click away in the title bar's menu
    // ("Why is the list incomplete?"). Operator, 2026-08-28.
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
