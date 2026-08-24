import * as vscode from "vscode";

import type { Conversation } from "./conversationList";

/// The scheme the tree gives its rows so that decorations can find them.
///
/// A scheme of its own rather than the conversation's folder path. Decoration providers are asked about every
/// uri in the window, and a row addressed by a real file path would collect whatever git and the problems view
/// have to say about that folder, on top of what this file says about the conversation.
export const CONVERSATION_SCHEME = "runtrol-conversation";

/// The uri that stands for one conversation row.
export function conversationUri(key: string): vscode.Uri {
  return vscode.Uri.from({ scheme: CONVERSATION_SCHEME, path: `/${encodeURIComponent(key)}` });
}

/// The decoration channel is reserved for the one exceptional row that cannot be opened. Ordinary state is not
/// repeated here: the tree uses a spinner while running and no state mark after it stops.
export class ConversationDecorations implements vscode.FileDecorationProvider {
  private readonly changed = new vscode.EventEmitter<vscode.Uri[]>();
  private openable = new Map<string, boolean>();

  readonly onDidChangeFileDecorations = this.changed.event;

  /// Take the rows the tree is about to show, and repaint only those whose openability changed.
  update(rows: readonly Conversation[]): void {
    const openable = new Map<string, boolean>();
    const moved: vscode.Uri[] = [];
    for (const row of rows) {
      openable.set(row.key, row.canOpen);
      if (this.openable.get(row.key) !== row.canOpen) {
        moved.push(conversationUri(row.key));
      }
    }
    for (const key of this.openable.keys()) {
      if (!openable.has(key)) moved.push(conversationUri(key));
    }
    this.openable = openable;
    if (moved.length > 0) this.changed.fire(moved);
  }

  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    if (uri.scheme !== CONVERSATION_SCHEME) return undefined;
    const key = decodeURIComponent(uri.path.replace(/^\//u, ""));
    if (this.openable.get(key) === false) {
      return {
        badge: "⊘",
        color: new vscode.ThemeColor("disabledForeground"),
        tooltip: "This coding service cannot reopen this conversation.",
        // Never bubbled to the project heading. A collapsible row is asked about its descendants, and a
        // propagating badge replaces the heading's own with a generic dot the reader cannot act on.
        propagate: false,
      };
    }
    return undefined;
  }

  dispose(): void {
    this.changed.dispose();
  }
}
