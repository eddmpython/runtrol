import * as vscode from "vscode";

import {
  attentionCount,
  loose,
  projectDetail,
  projects,
  type Conversation,
  type ProjectGroup,
} from "./conversationList";
import { ConversationDecorations, conversationUri } from "./conversationDecorations";
import type { ProjectRecord } from "./projects";
import { awaitsVerification, isUsable } from "./providerHealth";
import type { ProviderCapabilities } from "./runtimeTypes";
import { RuntimeState } from "./state";

/// One conversation, as one row.
///
/// The conversation title is the only text. Its coding-service glyph spins while it runs.
export class ConversationItem extends vscode.TreeItem {
  constructor(
    readonly conversation: Conversation,
    capabilities: ProviderCapabilities | null = null,
  ) {
    super(conversation.title, vscode.TreeItemCollapsibleState.None);
    this.id = conversation.key;
    // The row is one coding-service glyph and one human title. State and age never become a second label.
    this.description = undefined;
    this.contextValue = contextValue(conversation, capabilities);
    this.iconPath = icon(conversation);
    // What the badge attaches to. A scheme of its own so the row does not also collect whatever git and the
    // problems view have to say about the folder it happens to sit in.
    this.resourceUri = conversationUri(conversation.key);
    if (conversation.canOpen) {
      this.command = {
        command: "runtrol.selectSession",
        title: "Open conversation",
        arguments: [this],
      };
    }
    this.accessibilityInformation = {
      // The visible row keeps state out of its text, but readers still receive it through this label.
      label: `${conversation.title}, ${conversation.serviceName}, ${spokenActivity(conversation)}`,
    };
  }
}

/// One project heading: a folder the operator created a project on, has open in this window, or that a coding
/// service reports conversations in. The conversations beneath it are the rows.
///
/// The panel shows the whole machine's established projects (memory/uxContract.md). A one-off working directory
/// remains a plain conversation rather than becoming a project heading. The current open folder is the one empty
/// heading allowed without registration, because it is where the person opened Runtrol to work.
export class ProjectItem extends vscode.TreeItem {
  constructor(readonly group: ProjectGroup, agentToolsEnabled = false) {
    super(
      group.name,
      // A project with nothing in it yet has nothing to disclose, so it draws as a plain row whose only invite
      // is the new-conversation button. Otherwise: open when the reader has a reason to be looking inside
      // (something waiting, something live, the window's own project, or the open conversation lives here),
      // closed for everything else so twenty projects do not become one long scroll.
      group.rows.length === 0
        ? vscode.TreeItemCollapsibleState.None
        : group.attention > 0 || group.live > 0 || group.current || group.holdsOpen
          ? vscode.TreeItemCollapsibleState.Expanded
          : vscode.TreeItemCollapsibleState.Collapsed,
    );
    this.id = group.key;
    const detail = projectDetail(group);
    this.description = [detail, agentToolsEnabled ? "Agent Tools" : ""].filter(Boolean).join(" · ");
    // What the heading offers depends on why it exists and whether it is this window. The move button draws
    // only on headings that are not this window: opening the folder you are already in is not a move, and the
    // contract (memory/uxContract.md) wants moving to be the one explicit act. Rename and remove belong to
    // created projects; a discovered or open folder offers "make this a project" instead.
    this.contextValue = projectContextValue(group);
    // Keep the real folder in the tooltip and command payload, but do not expose it as the tree resource. VS Code's
    // Git decorations would otherwise append an unrelated dirty-file badge to the project heading.
    this.tooltip = agentToolsEnabled
      ? `${group.workspace}\nAgent Tools enabled for this project`
      : group.workspace;
    this.iconPath = group.attention > 0
      ? new vscode.ThemeIcon("folder", new vscode.ThemeColor("notificationsWarningIcon.foreground"))
      : new vscode.ThemeIcon(group.current ? "folder-opened" : "folder");
    this.accessibilityInformation = {
      label: `${group.name}${group.current ? ", this window's project" : ""}${this.description ? `, ${this.description}` : ""}`,
    };
  }
}

/// The context value the menus key on: `runtrol.project.<kind>` plus `.current` for this window's own folder.
function projectContextValue(group: ProjectGroup): string {
  return `runtrol.project.${group.kind}${group.current ? ".current" : ""}`;
}

export type ChatTreeItem = ConversationItem | ProjectItem;

/// Where the tree learns which projects the operator has created, without owning their storage.
export type ProjectsPort = {
  all(): readonly ProjectRecord[];
  onDidChange(listener: () => void): { dispose(): void };
};

export type AgentToolsPort = {
  enabled(workspace: string): boolean;
  onDidChange(listener: () => void): { dispose(): void };
};

/*
 * The provider glyph always identifies the coding service. While work is actually running, the same glyph spins.
 */
function icon(conversation: Conversation): vscode.ThemeIcon {
  if (!conversation.canOpen) {
    return new vscode.ThemeIcon(conversation.serviceIcon, new vscode.ThemeColor("disabledForeground"));
  }
  return conversation.activity === "working"
    ? new vscode.ThemeIcon(`${conversation.serviceIcon}~spin`)
    : new vscode.ThemeIcon(conversation.serviceIcon);
}

function spokenActivity(conversation: Conversation): string {
  if (!conversation.canOpen) return "cannot be reopened";
  switch (conversation.activity) {
    case "needsYou":
      return "needs you";
    case "attention":
      return "needs attention";
    case "working":
      return "working";
    case "waitingOnQuota":
      return "waiting on a limit";
    case "ready":
      return "ready";
    case "saved":
      return "saved";
  }
}

function contextValue(
  conversation: Conversation,
  capabilities: ProviderCapabilities | null,
): string {
  const mutationSuffix = conversation.native
    ? (capabilities?.nativeSessionArchive?.availability === "available" ? ".archive" : "")
      + (capabilities?.nativeSessionDelete?.availability === "available" ? ".delete" : "")
    : "";
  if (!conversation.canOpen) return `runtrol.conversation.blocked${mutationSuffix}`;
  if (!conversation.session) return `runtrol.conversation.saved${mutationSuffix}`;
  // Suffixes the row's inline actions key on: a pending question gets allow and decline, a sign-in need gets
  // the service's own sign-in line. Menus match these by prefix, so the base value stays stable.
  const suffix = (conversation.activity === "needsYou" ? ".needsYou" : "")
    + (conversation.signInNeeded ? ".signIn" : "");
  return `runtrol.conversation.live${suffix}${mutationSuffix}`;
}

function tooltip(conversation: Conversation): vscode.MarkdownString {
  const lines = [
    `**${conversation.title}**`,
    "",
    `${conversation.serviceName} · ${spokenActivity(conversation)}`,
    conversation.projectless ? "No project" : conversation.workspace,
  ];
  if (conversation.blocked) lines.push("", conversation.blocked);
  const markdown = new vscode.MarkdownString(lines.join("\n\n"));
  markdown.supportThemeIcons = true;
  return markdown;
}

/// The entry point. Everything a person can reach lives in this one list.
export class ConversationsTree implements vscode.TreeDataProvider<ChatTreeItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<ChatTreeItem | undefined>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private readonly subscription: vscode.Disposable;
  private readonly projectSubscription: { dispose(): void };
  private readonly agentToolsSubscription: { dispose(): void } | null;
  private readonly workspaceSubscription: vscode.Disposable;
  private items: ChatTreeItem[] | undefined;
  /// The heading each conversation belongs under, by conversation key. Empty while the list is flat.
  private parents: Map<string, ProjectItem> | undefined;
  /// Rows already built for a heading, built the first time that heading is opened and not before.
  ///
  /// Measured: eagerly building every heading's rows cost real time on a machine with thirty projects, because
  /// twenty-nine of those headings are closed and nobody was going to look at them. VS Code only asks for the
  /// children of what it draws, so building them any earlier is work performed for no reader.
  private readonly built = new Map<string, ConversationItem[]>();
  /// The conversations under each heading, as data rather than as tree items.
  private grouped: Map<string, readonly Conversation[]> | undefined;
  /// The top-level conversation rows: the loose ones sitting beside the headings.
  private flat: ConversationItem[] | undefined;
  /// The exceptional badge for a row that cannot perform the ordinary open action.
  ///
  /// Owned here because it is fed from the same rows the tree draws, and a second place computing which
  /// conversations exist would be a second answer to that question.
  readonly decorations = new ConversationDecorations();
  private view: vscode.TreeView<ChatTreeItem> | null = null;
  private badged: number | null = null;
  /// Whether the provider-owned listing explanation action is currently relevant.
  private listingIncomplete: boolean | null = null;
  private usableProvider: boolean | null = null;
  private verifyingProvider: boolean | null = null;
  private revealed: string | null = null;
  private revealedCurrentProject: string | null = null;

  constructor(
    private readonly state: RuntimeState,
    private readonly projectRecords: ProjectsPort,
    private readonly agentTools: AgentToolsPort | null = null,
  ) {
    this.projectSubscription = projectRecords.onDidChange(() => {
      this.forgetItems();
      this.changedEmitter.fire(undefined);
    });
    this.agentToolsSubscription = agentTools?.onDidChange(() => {
      this.forgetItems();
      this.changedEmitter.fire(undefined);
    }) ?? null;
    this.workspaceSubscription = vscode.workspace.onDidChangeWorkspaceFolders(() => {
      this.revealedCurrentProject = null;
      this.forgetItems();
      this.changedEmitter.fire(undefined);
      this.revealCurrentProject();
    });
    this.subscription = state.onDidChange((change) => {
      if (change === "selection") {
        // Selection changes which row is scrolled to, not what any row says. No `ConversationItem` reads
        // `open`: the label, detail, glyph, context value and command are all computed from state the
        // selection does not touch. So rebuilding every row here would be an allocation per conversation and
        // a full repaint in order to render exactly what was already on screen, on the one interaction a
        // person feels most: switching conversations.
        //
        // It is also the safer half. Reusing the items VS Code already knows is what makes revealing one
        // resolvable; handing it freshly built objects for the same rows is how a reveal starts failing.
        this.revealed = null;
        this.revealOpenConversation();
        return;
      }
      this.forgetItems();
      this.changedEmitter.fire(undefined);
      this.updateBadge();
      this.updateDiscoveryNotice();
      this.updateWelcomeContext();
      this.revealCurrentProject();
    });
  }

  bindView(view: vscode.TreeView<ChatTreeItem>): void {
    this.view = view;
    this.updateBadge();
    this.updateDiscoveryNotice();
    this.updateWelcomeContext();
    this.revealCurrentProject();
    this.revealOpenConversation();
  }

  getTreeItem(element: ChatTreeItem): vscode.TreeItem {
    return element;
  }

  /// Build the hover only for the row actually being hovered.
  ///
  /// A tooltip is markdown, and thirty of them were being constructed on every session-index change purely so that
  /// one of them might be read. VS Code asks for the hover when it needs it, which is the only time it is worth
  /// paying for.
  resolveTreeItem(item: vscode.TreeItem, element: ChatTreeItem): vscode.TreeItem {
    if (element instanceof ConversationItem) {
      item.tooltip = tooltip(element.conversation);
    }
    return item;
  }

  /// The heading a row lives under, which is what makes revealing it work.
  ///
  /// This has to be exact. VS Code resolves a reveal by walking parents, and a provider that answers
  /// `undefined` for a nested row makes every reveal fail with "Cannot resolve tree item" while the list
  /// itself still looks perfect. That failure was measured here once already, from a different cause, and it
  /// cost six times the session-switch budget in reveal retries.
  getParent(element: ChatTreeItem): ChatTreeItem | undefined {
    if (!(element instanceof ConversationItem)) return undefined;
    this.ensureItems();
    return this.parents?.get(element.conversation.key);
  }

  getChildren(element?: ChatTreeItem): ChatTreeItem[] {
    this.ensureItems();
    if (element instanceof ProjectItem) {
      return this.rowsUnder(element.group.key);
    }
    if (element) return [];
    return this.items ?? [];
  }

  dispose(): void {
    this.view = null;
    this.forgetItems();
    this.subscription.dispose();
    this.projectSubscription.dispose();
    this.agentToolsSubscription?.dispose();
    this.workspaceSubscription.dispose();
    this.decorations.dispose();
    this.changedEmitter.dispose();
  }

  /// Drop the built tree. One method so a new cache cannot be added and left stale in one of two places.
  private forgetItems(): void {
    this.items = undefined;
    this.parents = undefined;
    this.grouped = undefined;
    this.flat = undefined;
    this.built.clear();
  }

  /// The count on the activity bar icon, so a blocked agent is visible from a different view entirely.
  /// Say, above the list, when the list is not everything.
  ///
  /// The services answer this themselves and the sentence is theirs ("some stored conversations
  /// name no folder and are not shown"). A reader who cannot find yesterday's
  /// conversation and is told nothing concludes it is gone and leaves for another tool, so the
  /// qualification belongs where the list is, not in a log. The view's own message area is used
  /// rather than a row, because a row would sort, count, and be clickable, and this is none of those.
  /// The services' own reasons the list is not everything, for the (i) in the title.
  listingReasons(): string | null {
    return this.state.incompleteDiscovery;
  }

  private updateDiscoveryNotice(): void {
    const view = this.view;
    if (!view) return;
    const incomplete = this.state.incompleteDiscovery !== null;
    if (incomplete === this.listingIncomplete) return;
    this.listingIncomplete = incomplete;
    // Coverage diagnostics stay behind the title's information action. A permanent sentence above every
    // conversation made provider internals the first thing a reader saw and displaced the list itself.
    view.message = undefined;
    void vscode.commands.executeCommand("setContext", "runtrol.listingIncomplete", incomplete);
  }

  /// Distinguish a healthy first run from a machine with no usable coding service.
  ///
  /// Without this context both empty states received the same welcome, so a freshly installed and working CLI with
  /// no conversations was incorrectly reported as missing. The welcome now gives the exact next action for each case.
  private updateWelcomeContext(): void {
    const usable = this.state.providers.some(isUsable);
    const verifying = !usable && this.state.providers.some(awaitsVerification);
    if (usable === this.usableProvider && verifying === this.verifyingProvider) return;
    this.usableProvider = usable;
    this.verifyingProvider = verifying;
    void vscode.commands.executeCommand("setContext", "runtrol.hasUsableProvider", usable);
    void vscode.commands.executeCommand("setContext", "runtrol.isVerifyingProvider", verifying);
  }

  private updateBadge(): void {
    const view = this.view;
    if (!view) return;
    const waiting = attentionCount(this.state.conversations);
    // Writing the same badge again is a repaint nobody asked for, and the index changes constantly.
    if (waiting === this.badged) return;
    this.badged = waiting;
    view.badge = waiting === 0
      ? undefined
      : {
        value: waiting,
        tooltip: waiting === 1
          ? "1 conversation is waiting for you"
          : `${waiting} conversations are waiting for you`,
      };
  }

  /// Select one conversation's row (the harness: the row's inline actions show on a selected row).
  async revealSession(sessionId: string): Promise<void> {
    const view = this.view;
    if (!view) return;
    this.ensureItems();
    const conversation = this.state.conversationOf(sessionId);
    const row = conversation ? this.rowFor(conversation.key) : null;
    if (!row) return;
    await view.reveal(row, { select: true, focus: true });
  }

  private revealOpenConversation(): void {
    const view = this.view;
    if (!view || !view.visible) return;
    this.ensureItems();
    // Asks the data which conversation is open before building any tree item, so a window with thirty closed
    // projects builds exactly one heading's rows rather than all of them.
    const conversation = this.openConversation();
    if (!conversation || conversation.key === this.revealed) return;
    const open = this.rowFor(conversation.key);
    if (!open) return;
    this.revealed = open.conversation.key;
    // Revealing is cosmetic, and it races the refresh that was just announced. A failure here must stay a
    // swallowed nicety rather than an unhandled rejection: the row is still correct, it just is not scrolled to.
    view.reveal(open, { select: true, focus: false }).then(undefined, () => {
      this.revealed = null;
    });
  }

  private ensureItems(): void {
    if (this.items) return;
    const rows = this.state.conversations;
    // Before anything is built, so a row drawn in this pass already has its badge.
    this.decorations.update(rows);
    const records = this.projectRecords.all();
    const openWorkspaces = this.openWorkspaces();
    const groups = projects(records, rows, openWorkspaces);
    // Beneath the headings, not under one. A conversation started with no project is still a conversation.
    const unfiled = loose(rows, records, openWorkspaces).map((row) => new ConversationItem(
      row,
      this.state.providerCapabilities(row.providerId),
    ));
    const parents = new Map<string, ProjectItem>();
    const grouped = new Map<string, readonly Conversation[]>();

    if (groups.length === 0 && unfiled.length === 0) {
      // No conversations at all, filed or otherwise. The welcome content covers that case.
      this.items = [];
      this.flat = [];
      this.parents = parents;
      this.grouped = grouped;
      return;
    }

    const headings: ProjectItem[] = [];
    for (const group of groups) {
      const heading = new ProjectItem(group, this.agentTools?.enabled(group.workspace) ?? false);
      headings.push(heading);
      grouped.set(group.key, group.rows);
      // The parent map is the cheap half and reveal needs it immediately, so it is built now. The rows
      // themselves wait until something asks to draw them.
      for (const row of group.rows) {
        parents.set(row.key, heading);
      }
    }
    this.items = [...headings, ...unfiled];
    // The loose rows are top-level items, so revealing one resolves against these exact objects.
    this.flat = unfiled;
    this.parents = parents;
    this.grouped = grouped;
  }

  /// The rows under one heading, built on first sight and kept.
  private rowsUnder(key: string): ConversationItem[] {
    const already = this.built.get(key);
    if (already) return already;
    const under = (this.grouped?.get(key) ?? []).map(
      (row) => new ConversationItem(
        row,
        this.state.providerCapabilities(row.providerId),
      ),
    );
    this.built.set(key, under);
    return under;
  }

  /// The tree item for one conversation, wherever it currently lives.
  private rowFor(key: string): ConversationItem | undefined {
    const flat = this.flat?.find((item) => item.conversation.key === key);
    if (flat) return flat;
    const heading = this.parents?.get(key);
    if (!heading) return undefined;
    return this.rowsUnder(heading.group.key).find((item) => item.conversation.key === key);
  }

  /// The conversation this window should be scrolled to, as data.
  private openConversation(): Conversation | undefined {
    for (const row of this.state.conversations) {
      if (row.open) return row;
    }
    return undefined;
  }

  /// The folders this window is open on, which is what makes one project the reader's current one.
  private openWorkspaces(): string[] {
    return (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
  }

  /// Expand this window's project once its conversations arrive.
  ///
  /// The current folder is first even before discovery finishes. VS Code remembers the initial empty row as
  /// collapsed when its children arrive later, so an explicit one-time reveal keeps the work at hand visible
  /// without reopening it after the person deliberately collapses it.
  private revealCurrentProject(): void {
    const view = this.view;
    if (!view || !view.visible) return;
    this.ensureItems();
    const current = this.items?.find(
      (item): item is ProjectItem => item instanceof ProjectItem && item.group.current && item.group.rows.length > 0,
    );
    if (!current || current.group.key === this.revealedCurrentProject) return;
    view.reveal(current, { expand: true, select: false, focus: false }).then(() => {
      this.revealedCurrentProject = current.group.key;
    }, () => undefined);
  }

}
