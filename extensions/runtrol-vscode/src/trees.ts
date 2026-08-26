import * as vscode from "vscode";

import { conversationIcon } from "./conversationIcon";
import { canDelete } from "./conversationDeletion";

import {
  attentionCount,
  loose,
  projectDetail,
  projects,
  type Conversation,
  type ProjectGroup,
} from "./conversationList";
import { ConversationDecorations, conversationUri } from "./conversationDecorations";
import { projectColorId } from "./projectColor";
import type { ProjectRecord } from "./projects";
import { awaitsVerification, isUsable } from "./providerHealth";
import type { ProviderCapabilities } from "./runtimeTypes";
import { type CoreReach, RuntimeState } from "./state";

/// One conversation, as one row.
///
/// The conversation title is the only text. Its coding-service glyph spins while it runs.
export class ConversationItem extends vscode.TreeItem {
  constructor(
    readonly conversation: Conversation,
    capabilities: ProviderCapabilities | null = null,
    extensionUri: vscode.Uri | null = null,
  ) {
    super(conversation.title, vscode.TreeItemCollapsibleState.None);
    this.id = conversation.key;
    // The row is one coding-service glyph and one human title. State and age never become a second label.
    this.description = undefined;
    this.contextValue = contextValue(conversation, capabilities);
    this.iconPath = icon(conversation, extensionUri);
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

/// One project heading: a folder the operator added as a project, or has open in this window. The conversations
/// beneath it are the rows.
///
/// A project is a decision, never a discovery (`docs/vscodeSurface.md`). A conversation in a folder nobody added
/// stays a plain top-level row. The current open folder is the one heading allowed without adding, because it is
/// where the person opened Runtrol to work.
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
    // contract (`docs/vscodeSurface.md`) wants moving to be the one explicit act. Rename, pin and remove belong
    // to added projects; the open folder offers "add this folder as a project" instead.
    this.contextValue = projectContextValue(group, agentToolsEnabled);
    // Keep the real folder in the tooltip and command payload, but do not expose it as the tree resource. VS Code's
    // Git decorations would otherwise append an unrelated dirty-file badge to the project heading.
    this.tooltip = agentToolsEnabled
      ? `${group.workspace}\nAgent Tools enabled for this project`
      : group.workspace;
    // The project's own colour, which its conversation tabs carry too, so a tab and its heading are read as one
    // thing. Something waiting outranks it: an unanswered agent is the one state that has to break the pattern.
    const colour = projectColorId(group.workspace);
    this.iconPath = group.attention > 0
      ? new vscode.ThemeIcon("folder", new vscode.ThemeColor("notificationsWarningIcon.foreground"))
      : new vscode.ThemeIcon(
        group.pinned ? "pinned" : group.current ? "folder-opened" : "folder",
        colour ? new vscode.ThemeColor(colour) : undefined,
      );
    this.accessibilityInformation = {
      label: `${group.name}${group.current ? ", this window's project" : ""}${this.description ? `, ${this.description}` : ""}`,
    };
  }
}

/// The context value the menus key on: `runtrol.project.<kind>`, plus `.current` for this window's own folder,
/// plus `.pinned` or `.pinnable` on an added project so its inline button offers the one of pin and unpin that
/// applies.
function projectContextValue(group: ProjectGroup, agentToolsEnabled: boolean): string {
  const pin = group.kind === "created" ? (group.pinned ? ".pinned" : ".pinnable") : "";
  // The row says whether its tools are on, so the menu offers the one action that applies. Without this the
  // same folder carried both "enable" and "disable" at once, which asks the reader to know the state the row
  // was supposed to tell them.
  const tools = agentToolsEnabled ? ".tools" : ".noTools";
  return `runtrol.project.${group.kind}${group.current ? ".current" : ""}${tools}${pin}`;
}

/// One coding service, offered where the person pressed the button.
///
/// The choice belongs beside the list, not at the top of the window: a picker up there drags the eye off the
/// section the person was working in, and they have to come back down to see what happened. These rows are
/// built from the services the Core reports, so a service added by manifest appears here without this file
/// knowing its name.
export class ServiceChoiceItem extends vscode.TreeItem {
  constructor(
    readonly providerId: string,
    readonly workspace: string,
    displayName: string,
    icon: string,
    extensionUri: vscode.Uri | null,
  ) {
    super(displayName, vscode.TreeItemCollapsibleState.None);
    this.id = `choose:${workspace}:${providerId}`;
    this.iconPath = extensionUri ? conversationIcon(extensionUri, icon) : new vscode.ThemeIcon(icon);
    this.contextValue = "runtrol.serviceChoice";
    this.command = {
      command: "runtrol.startSessionWith",
      title: "Start a conversation with this service",
      arguments: [this],
    };
  }
}

export type ChatTreeItem = ConversationItem | ProjectItem | ServiceChoiceItem;

/// Which half of the sidebar one tree draws.
///
/// The sidebar is two sections in a fixed order (`docs/vscodeSurface.md`): Projects, then the
/// conversations that belong to no project. VS Code makes a section a view, so this is one provider stood up
/// twice rather than two providers that would answer "what conversations exist" separately.
export type SidebarPart = "projects" | "loose";

/// Pinned conversations first, in the order the list already had. Pinning is the person's own placement, so
/// it lifts a row inside its own section and never out of it.
function pinnedFirst(rows: readonly Conversation[]): Conversation[] {
  return [...rows.filter((row) => row.pinned), ...rows.filter((row) => !row.pinned)];
}

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
/// The row's service glyph.
///
/// The same shipped SVG the conversation tab and the usage strip draw, rather than a name looked up in the
/// editor's icon font. The font carries a mark for some services and none for others, so a font lookup left a
/// blank where Cline, OpenCode and Grok should be; the shipped folder has a mark for every service the build
/// knows, because the build writes one per manifest.
///
/// While a conversation is running the row shows motion instead, because only the editor's own glyphs can spin.
export function icon(conversation: Conversation, extensionUri: vscode.Uri | null): vscode.ThemeIcon | vscode.Uri {
  if (conversation.activity === "working") {
    return new vscode.ThemeIcon("sync~spin");
  }
  if (!extensionUri) {
    return new vscode.ThemeIcon(conversation.serviceIcon);
  }
  return conversationIcon(extensionUri, conversation.serviceIcon);
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
  // Archive stays a native-only action; delete asks the one shared truth, so an orphan pointer (supervised,
  // no native identity) gets its delete button too. Measured 2026-08-25: two such rows sat undeletable
  // because the affordance keyed on a native identity they did not have while the click would have forgotten
  // them.
  const mutationSuffix = (conversation.native
    && capabilities?.nativeSessionArchive?.availability === "available" ? ".archive" : "")
    + (canDelete(conversation, capabilities) ? ".delete" : "");
  // Every conversation can be pinned; the token says which of pin and unpin the row's inline button offers.
  const pinState = conversation.pinned ? ".pinned" : ".pinnable";
  if (!conversation.canOpen) return `runtrol.conversation.blocked${mutationSuffix}${pinState}`;
  if (!conversation.session) return `runtrol.conversation.saved${mutationSuffix}${pinState}`;
  // Suffixes the row's inline actions key on: a pending question gets allow and decline, a sign-in need gets
  // the service's own sign-in line. Menus match these by prefix, so the base value stays stable.
  const suffix = (conversation.activity === "needsYou" ? ".needsYou" : "")
    + (conversation.signInNeeded ? ".signIn" : "");
  return `runtrol.conversation.live${suffix}${mutationSuffix}${pinState}`;
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
  private reach: CoreReach | null = null;
  private revealed: string | null = null;
  private revealedCurrentProject: string | null = null;
  /// The folder a service is being chosen for, or null when nothing is being chosen. Held here because the
  /// choice is drawn as rows in this very view.
  private choosingFor: string | null = null;
  private services: (() => readonly { providerId: string; displayName: string; icon: string }[]) | null = null;

  constructor(
    private readonly part: SidebarPart,
    private readonly state: RuntimeState,
    private readonly projectRecords: ProjectsPort,
    private readonly agentTools: AgentToolsPort | null = null,
    private readonly extensionUri: vscode.Uri | null = null,
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
      this.updateCoreNotice();
      this.updateWelcomeContext();
      this.revealCurrentProject();
    });
  }

  bindView(view: vscode.TreeView<ChatTreeItem>): void {
    this.view = view;
    this.updateBadge();
    this.updateDiscoveryNotice();
    this.updateCoreNotice();
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

  /// Where the services come from, so this file never names one.
  offerServices(
    services: () => readonly { providerId: string; displayName: string; icon: string }[],
  ): void {
    this.services = services;
  }

  /// Draw the service choice for one folder, in this section, until something is chosen.
  chooseService(workspace: string): void {
    this.choosingFor = workspace;
    this.forgetItems();
    this.changedEmitter.fire(undefined);
  }

  /// Put the choice away again.
  clearServiceChoice(): void {
    if (this.choosingFor === null) return;
    this.choosingFor = null;
    this.forgetItems();
    this.changedEmitter.fire(undefined);
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
    void vscode.commands.executeCommand("setContext", "runtrol.listingIncomplete", incomplete);
  }

  /// Say, above the list, when the list cannot be trusted to be everything.
  ///
  /// Coverage diagnostics deliberately stay behind the title's information action: a permanent sentence about
  /// provider internals displaced the list itself. Losing the Core is not a diagnostic. Measured 2026-08-26:
  /// with the Core unreachable the tree still drew the open folder's heading, so it was never empty, so the
  /// view's welcome never appeared, and the sidebar showed one silent empty project with no reason given. The
  /// only place that said anything was the usage strip, in words about our own internals.
  private updateCoreNotice(): void {
    const view = this.view;
    if (!view) return;
    view.message = this.state.coreReach === "unreachable"
      ? "Cannot reach the Runtrol Core. Your conversations are still on this machine; this window is trying again."
      : undefined;
  }

  /// Distinguish a healthy first run from a machine with no usable coding service, and both of those from a
  /// Core this window cannot reach.
  ///
  /// Without this context every empty list received the same welcome, so a freshly installed and working CLI
  /// with no conversations was reported as missing, and later a dropped connection was reported as a machine
  /// with nothing installed at all (measured on the operator's window 2026-08-26 with three services running).
  /// The welcome now gives the exact next action for each case.
  private updateWelcomeContext(): void {
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

  /// Every identity this tree currently holds, at the top level and beneath each heading.
  ///
  /// A tree refuses two items that carry the same identity, and a conversation drawn in two places would carry
  /// one identity twice. Building the whole panel and reading its identities back is how the harness asserts
  /// that lifting a pinned conversation to the top never leaves a copy of it under its project.
  treeItemIdsForJourney(): string[] {
    this.ensureItems();
    const ids: string[] = [];
    for (const item of this.items ?? []) {
      if (typeof item.id === "string") ids.push(item.id);
      if (item instanceof ProjectItem) {
        for (const row of this.rowsUnder(item.group.key)) {
          if (typeof row.id === "string") ids.push(row.id);
        }
      }
    }
    return ids;
  }

  /// Select one conversation's row by its conversation key, so a stored conversation with no running session
  /// can be brought forward and show its inline actions (rename, pin, delete) the same way a live row does.
  async revealConversation(key: string): Promise<void> {
    const view = this.view;
    if (!view) return;
    this.ensureItems();
    const row = this.rowFor(key);
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
    // Pinned conversations lead the whole panel, above every heading. Pinning says "keep this where I can see
    // it", and a pinned row that sorts only within its own section is below thirty headings, which is not what
    // the person asked for. They keep their heading too, so the project still counts and lists them.
    const parents = new Map<string, ProjectItem>();
    const grouped = new Map<string, readonly Conversation[]>();

    // A choice in progress leads its own section, so the person's eye stays where they pressed the button.
    const choices = this.choosingFor === null
      ? []
      : (this.services?.() ?? []).map((service) => new ServiceChoiceItem(
        service.providerId,
        this.choosingFor as string,
        service.displayName,
        service.icon,
        this.extensionUri,
      ));

    if (this.part === "loose") {
      // Conversations that belong to no project. A folder nobody added contributes none of these, so this
      // section is only ever what the person started without choosing a place (`loose`).
      const rowsHere = pinnedFirst(loose(rows)).map((row) => new ConversationItem(
        row,
        this.state.providerCapabilities(row.providerId),
        this.extensionUri,
      ));
      this.items = [...choices, ...rowsHere];
      this.flat = rowsHere;
      this.parents = parents;
      this.grouped = grouped;
      return;
    }

    const groups = projects(records, rows, openWorkspaces);
    const headings: ProjectItem[] = [];
    for (const group of groups) {
      const heading = new ProjectItem(group, this.agentTools?.enabled(group.workspace) ?? false);
      headings.push(heading);
      // Pinned conversations lead their own project rather than the whole panel. With the sidebar split in
      // two, lifting a row above every heading would take it out of the place it belongs to, and one
      // conversation drawn in two places carries the same identity, which a tree refuses.
      grouped.set(group.key, pinnedFirst(group.rows));
      // The parent map is the cheap half and reveal needs it immediately, so it is built now. The rows
      // themselves wait until something asks to draw them.
      for (const row of group.rows) parents.set(row.key, heading);
    }
    this.items = [...choices, ...headings];
    this.flat = [];
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
        this.extensionUri,
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
