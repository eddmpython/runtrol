import * as vscode from "vscode";

import {
  attentionCount,
  conversationDetail,
  projectDetail,
  projects,
  type Conversation,
  type ProjectGroup,
} from "./conversationList";
import { isBroken } from "./providerHealth";
import type { ProviderLine } from "./runtimeTypes";
import { RuntimeState } from "./state";

/// One conversation, as one row.
///
/// There is no second kind of row. A coding service is a fact about a conversation, not a container for one, so it
/// appears in the muted detail line rather than as a node the reader has to open first.
export class ConversationItem extends vscode.TreeItem {
  constructor(readonly conversation: Conversation, nowMs: number) {
    super(conversation.title, vscode.TreeItemCollapsibleState.None);
    this.id = conversation.key;
    // A conversation that stopped for the reader says so first. The service and the folder are what distinguish
    // rows from each other; this is what distinguishes one row from every other thing they could be doing.
    const detail = conversationDetail(conversation, nowMs);
    this.description = conversation.activity === "needsYou"
      ? `Needs you · ${detail}`
      : detail;
    this.contextValue = contextValue(conversation);
    this.iconPath = icon(conversation);
    if (conversation.canOpen) {
      this.command = {
        command: "runtrol.selectSession",
        title: "Open conversation",
        arguments: [this],
      };
    }
    this.accessibilityInformation = {
      label: `${conversation.title}, ${spokenActivity(conversation)}, ${this.description}`,
    };
  }
}

/// The one row that is not a conversation, shown only when something is actually wrong.
///
/// A coding service that is simply not installed is not a problem and gets no row. Discovery decides what exists,
/// and a list of things the reader does not have is not a list they asked for.
export class ServiceProblemItem extends vscode.TreeItem {
  constructor(provider: ProviderLine) {
    super(`${provider.displayName} needs attention`, vscode.TreeItemCollapsibleState.None);
    this.id = `runtrol.problem.${encodeURIComponent(provider.providerId)}`;
    this.description = "Unavailable";
    this.tooltip = provider.installation.why ?? `${provider.displayName} cannot currently start a conversation.`;
    this.contextValue = "runtrol.serviceProblem";
    this.iconPath = new vscode.ThemeIcon(
      "warning",
      new vscode.ThemeColor("problemsWarningIcon.foreground"),
    );
  }
}

/// One project, holding the conversations that belong to it.
///
/// Only ever shown when there is more than one project to tell apart. A single heading over the whole list
/// is a disclosure triangle that costs a click and separates nothing.
export class ProjectItem extends vscode.TreeItem {
  constructor(readonly group: ProjectGroup) {
    super(
      group.name,
      // Anything with a conversation waiting is open, whatever else is true, because the reader must not have
      // to guess which heading hides the thing that stopped. Then this window's own project, because that is
      // where they already are. Everything else stays closed so ten projects do not become one long scroll.
      group.attention > 0 || group.current
        ? vscode.TreeItemCollapsibleState.Expanded
        : vscode.TreeItemCollapsibleState.Collapsed,
    );
    this.id = group.key;
    this.description = projectDetail(group);
    this.contextValue = "runtrol.project";
    this.resourceUri = vscode.Uri.file(group.workspace);
    this.iconPath = group.attention > 0
      ? new vscode.ThemeIcon("folder", new vscode.ThemeColor("notificationsWarningIcon.foreground"))
      : new vscode.ThemeIcon(group.current ? "folder-opened" : "folder");
    this.accessibilityInformation = {
      label: `${group.name}${group.current ? ", this window's project" : ""}, ${this.description}`,
    };
  }
}

export type ChatTreeItem = ConversationItem | ServiceProblemItem | ProjectItem;

/*
 * One glyph per state, and the same glyph everywhere that state is shown.
 *
 * The point of the vocabulary is that a list of eight running agents can be read without opening any of them. That
 * only works if the reader learns six shapes once, so no state may borrow another's glyph and none may be silent.
 */
function icon(conversation: Conversation): vscode.ThemeIcon {
  if (!conversation.canOpen) {
    return new vscode.ThemeIcon("circle-slash", new vscode.ThemeColor("disabledForeground"));
  }
  switch (conversation.activity) {
    case "needsYou":
      return new vscode.ThemeIcon("question", new vscode.ThemeColor("notificationsWarningIcon.foreground"));
    case "attention":
      return new vscode.ThemeIcon("error", new vscode.ThemeColor("problemsErrorIcon.foreground"));
    case "working":
      return new vscode.ThemeIcon("loading~spin", new vscode.ThemeColor("charts.orange"));
    case "waitingOnQuota":
      return new vscode.ThemeIcon("watch", new vscode.ThemeColor("descriptionForeground"));
    case "ready":
      return new vscode.ThemeIcon("circle-filled", new vscode.ThemeColor("charts.green"));
    case "saved":
      return new vscode.ThemeIcon("circle-outline");
  }
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

function contextValue(conversation: Conversation): string {
  if (!conversation.canOpen) return "runtrol.conversation.blocked";
  return conversation.session ? "runtrol.conversation.live" : "runtrol.conversation.saved";
}

function tooltip(conversation: Conversation): vscode.MarkdownString {
  const lines = [
    `**${conversation.title}**`,
    "",
    `${conversation.serviceName} · ${spokenActivity(conversation)}`,
    conversation.workspace,
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
  /// Conversation rows for the flat shape, where there is nothing to defer.
  private flat: ConversationItem[] | undefined;
  private view: vscode.TreeView<ChatTreeItem> | null = null;
  private badged: number | null = null;
  private revealed: string | null = null;

  constructor(
    private readonly state: RuntimeState,
    private readonly now: () => number = () => Date.now(),
  ) {
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
    });
  }

  bindView(view: vscode.TreeView<ChatTreeItem>): void {
    this.view = view;
    this.updateBadge();
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
    const nowMs = this.now();
    const rows = this.state.conversations;
    const groups = projects(rows, this.openWorkspaces());
    const problems = this.state.providers
      .filter(isBroken)
      .map((provider) => new ServiceProblemItem(provider));
    const parents = new Map<string, ProjectItem>();
    const grouped = new Map<string, readonly Conversation[]>();

    if (groups.length === 0) {
      // One project, or none. Nothing to separate, so the list stays flat and costs no clicks.
      const flat = rows.map((row) => new ConversationItem(row, nowMs));
      this.items = [...flat, ...problems];
      this.flat = flat;
      this.parents = parents;
      this.grouped = grouped;
      return;
    }

    const headings: ProjectItem[] = [];
    for (const group of groups) {
      const heading = new ProjectItem(group);
      headings.push(heading);
      grouped.set(group.key, group.rows);
      // The parent map is the cheap half and reveal needs it immediately, so it is built now. The rows
      // themselves wait until something asks to draw them.
      for (const row of group.rows) {
        parents.set(row.key, heading);
      }
    }
    this.items = [...headings, ...problems];
    this.flat = undefined;
    this.parents = parents;
    this.grouped = grouped;
  }

  /// The rows under one heading, built on first sight and kept.
  private rowsUnder(key: string): ConversationItem[] {
    const already = this.built.get(key);
    if (already) return already;
    const nowMs = this.now();
    const under = (this.grouped?.get(key) ?? []).map((row) => new ConversationItem(row, nowMs));
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
}
