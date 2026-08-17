import * as vscode from "vscode";

import { attentionCount, conversationDetail, type Conversation } from "./conversationList";
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

export type ChatTreeItem = ConversationItem | ServiceProblemItem;

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
  private view: vscode.TreeView<ChatTreeItem> | null = null;
  private badged: number | null = null;
  private revealed: string | null = null;

  constructor(
    private readonly state: RuntimeState,
    private readonly now: () => number = () => Date.now(),
  ) {
    this.subscription = state.onDidChange((change) => {
      this.items = undefined;
      if (change === "selection") this.revealed = null;
      this.changedEmitter.fire(undefined);
      this.updateBadge();
      if (change === "selection") this.revealOpenConversation();
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

  getParent(): undefined {
    return undefined;
  }

  getChildren(element?: ChatTreeItem): ChatTreeItem[] {
    if (element) return [];
    this.ensureItems();
    return this.items ?? [];
  }

  dispose(): void {
    this.view = null;
    this.items = undefined;
    this.subscription.dispose();
    this.changedEmitter.dispose();
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
    const open = this.items?.find(
      (item): item is ConversationItem => item instanceof ConversationItem && item.conversation.open,
    );
    if (!open || open.conversation.key === this.revealed) return;
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
    this.items = [
      ...this.state.conversations.map((row) => new ConversationItem(row, nowMs)),
      ...this.state.providers
        .filter(isBroken)
        .map((provider) => new ServiceProblemItem(provider)),
    ];
  }
}
