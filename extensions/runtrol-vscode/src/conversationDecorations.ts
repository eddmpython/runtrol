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

/// What the badge says, per state.
///
/// One or two characters, because the platform drops the whole decoration when a badge is three graphemes and
/// says so only in a log. Chosen to be readable without colour: on the selected row the editor overrides the
/// decoration colour with `!important`, so a state carried by colour alone would vanish exactly where the reader
/// is looking.
const BADGE: Record<Conversation["activity"], string> = {
  needsYou: "?",
  attention: "!",
  working: "··",
  waitingOnQuota: "…",
  ready: "✓",
  saved: "",
};

/// What colour the badge and the row take, per state.
///
/// The same colours the row's own glyph uses, so a reader who has learned one has learned the other.
const COLOUR: Record<Conversation["activity"], string | null> = {
  needsYou: "notificationsWarningIcon.foreground",
  attention: "problemsErrorIcon.foreground",
  working: "charts.orange",
  waitingOnQuota: "descriptionForeground",
  ready: "charts.green",
  saved: null,
};

/// A word for the state, for the hover and for a screen reader.
const SPOKEN: Record<Conversation["activity"], string> = {
  needsYou: "waiting for you",
  attention: "stopped with a problem",
  working: "working",
  waitingOnQuota: "waiting on a limit",
  ready: "finished",
  saved: "saved",
};

/// The second visual channel on a conversation row.
///
/// The row's glyph says which coding service it is, which is what tells two rows apart at a glance and cannot be
/// said in two characters (two of the four services this drives begin with the same letter). That leaves the
/// state, which can: a mark on the right edge and a colour, both put there by this provider.
///
/// Cheap on purpose. The editor repaints a decorated row in place when this fires, without the tree rebuilding
/// its items, so a state changing on eight running conversations costs eight repaints rather than a full refresh.
export class ConversationDecorations implements vscode.FileDecorationProvider {
  private readonly changed = new vscode.EventEmitter<vscode.Uri[]>();
  private states = new Map<string, Conversation["activity"]>();
  private openable = new Map<string, boolean>();

  readonly onDidChangeFileDecorations = this.changed.event;

  /// Take the rows the tree is about to show, and repaint only the ones whose state actually moved.
  update(rows: readonly Conversation[]): void {
    const states = new Map<string, Conversation["activity"]>();
    const openable = new Map<string, boolean>();
    const moved: vscode.Uri[] = [];
    for (const row of rows) {
      states.set(row.key, row.activity);
      openable.set(row.key, row.canOpen);
      if (this.states.get(row.key) !== row.activity || this.openable.get(row.key) !== row.canOpen) {
        moved.push(conversationUri(row.key));
      }
    }
    for (const key of this.states.keys()) {
      if (!states.has(key)) moved.push(conversationUri(key));
    }
    this.states = states;
    this.openable = openable;
    if (moved.length > 0) this.changed.fire(moved);
  }

  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    if (uri.scheme !== CONVERSATION_SCHEME) return undefined;
    const key = decodeURIComponent(uri.path.replace(/^\//u, ""));
    const activity = this.states.get(key);
    if (activity === undefined) return undefined;
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
    const badge = BADGE[activity];
    const colour = COLOUR[activity];
    if (!badge && !colour) return undefined;
    return {
      badge: badge || undefined,
      color: colour ? new vscode.ThemeColor(colour) : undefined,
      tooltip: SPOKEN[activity],
      propagate: false,
    };
  }

  dispose(): void {
    this.changed.dispose();
  }
}
