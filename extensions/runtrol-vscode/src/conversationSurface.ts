import * as vscode from "vscode";

/// Where a conversation lives in the window.
///
/// A conversation is one thing and can be shown in any of VS Code's own places: an editor tab (the default,
/// the file-click grammar), the bottom panel beside the terminals, or the secondary side bar beside the
/// code. The places are VS Code's; Runtrol adds none of its own. Which place a conversation is in changes
/// nothing about the conversation: the same page, the same watch, the same chips.
export type Place = "tab" | "panel" | "sideBar";

export const PLACES: readonly Place[] = ["tab", "panel", "sideBar"];

/// The view ids VS Code knows the two non-tab places by (`package.json` contributes them, inside the
/// `runtrolPanel` and `runtrolSide` containers: a container id is `[a-zA-Z0-9_-]+`, measured on 1.132, where a
/// dotted id is silently dropped and its views land in the Explorer).
export const PANEL_VIEW_ID = "runtrol.conversationPanel";
export const SIDE_BAR_VIEW_ID = "runtrol.conversationSide";

export function viewIdOf(place: Exclude<Place, "tab">): string {
  return place === "panel" ? PANEL_VIEW_ID : SIDE_BAR_VIEW_ID;
}

export function placeOfViewId(viewId: string): Exclude<Place, "tab"> | null {
  if (viewId === PANEL_VIEW_ID) return "panel";
  if (viewId === SIDE_BAR_VIEW_ID) return "sideBar";
  return null;
}

/// What a conversation page needs from the thing that hosts it, whichever place that is.
///
/// This is the binding contract for a place: a webview to post to, visibility to pause the watch by, a
/// title to set, a way to bring it on screen, and two lifetime events. Adding a place means implementing
/// this once; nothing in the page, the bindings or the controller reads the place beyond this surface.
export interface ConversationSurface {
  readonly place: Place;
  readonly webview: vscode.Webview;
  /// On screen (its webview exists). Hidden places are reborn when shown again.
  readonly visible: boolean;
  /// A tab: VS Code's active editor in its group. A view: the same as visible.
  readonly active: boolean;
  /// The editor column a tab sits in; null for a view.
  readonly viewColumn: vscode.ViewColumn | null;
  title: string;
  /// Bring the surface on screen. A tab may be asked to move to a column while it does.
  reveal(preserveFocus: boolean, column?: vscode.ViewColumn): void;
  onDidChangeVisibility(listener: () => void): vscode.Disposable;
  onDidDispose(listener: () => void): vscode.Disposable;
  /// Let go of the surface. A tab closes; a view stays (it is the workbench's) and is emptied.
  dispose(): void;
}

/// An editor tab as a surface.
export function tabSurface(panel: vscode.WebviewPanel, iconPath: vscode.Uri): ConversationSurface {
  panel.iconPath = iconPath;
  return {
    place: "tab",
    get webview() {
      return panel.webview;
    },
    get visible() {
      return panel.visible;
    },
    get active() {
      return panel.active;
    },
    get viewColumn() {
      return panel.viewColumn ?? null;
    },
    get title() {
      return panel.title;
    },
    set title(value: string) {
      panel.title = value;
    },
    reveal(preserveFocus, column) {
      panel.reveal(column ?? panel.viewColumn ?? vscode.ViewColumn.Active, preserveFocus);
    },
    onDidChangeVisibility(listener) {
      return panel.onDidChangeViewState(() => listener());
    },
    onDidDispose(listener) {
      return panel.onDidDispose(listener);
    },
    dispose() {
      panel.dispose();
    },
  };
}

/// A workbench view (the bottom panel's or the secondary side bar's) as a surface.
///
/// The view is resolved once by VS Code and outlives every conversation shown in it. A conversation that
/// leaves the view detaches: the surface goes inert, its dispose listeners fire so the binding closes, and
/// the view itself stays put for the next conversation (or shows the empty sentence).
export function viewSurface(
  view: vscode.WebviewView,
  place: Exclude<Place, "tab">,
  emptied: () => void,
): ConversationSurface {
  const disposeListeners = new Set<() => void>();
  let detached = false;
  const listenerGuards: vscode.Disposable[] = [];
  const surface: ConversationSurface = {
    place,
    get webview() {
      return view.webview;
    },
    get visible() {
      return !detached && view.visible;
    },
    get active() {
      return !detached && view.visible;
    },
    viewColumn: null,
    get title() {
      return view.title ?? "";
    },
    set title(value: string) {
      if (!detached) view.title = value;
    },
    reveal(preserveFocus) {
      if (!detached) view.show(preserveFocus);
    },
    onDidChangeVisibility(listener) {
      const guard = view.onDidChangeVisibility(() => {
        if (!detached) listener();
      });
      listenerGuards.push(guard);
      return guard;
    },
    onDidDispose(listener) {
      disposeListeners.add(listener);
      const viewGuard = view.onDidDispose(() => {
        if (!detached) listener();
      });
      listenerGuards.push(viewGuard);
      return new vscode.Disposable(() => {
        disposeListeners.delete(listener);
        viewGuard.dispose();
      });
    },
    dispose() {
      if (detached) return;
      detached = true;
      for (const guard of listenerGuards) guard.dispose();
      listenerGuards.length = 0;
      for (const listener of disposeListeners) listener();
      disposeListeners.clear();
      emptied();
    },
  };
  return surface;
}

/// The page a place shows while no conversation is in it. A sentence, not a feature: the sidebar is where
/// conversations are chosen, and this only says so.
export function emptyPlaceHtml(place: Exclude<Place, "tab">): string {
  const where = place === "panel" ? "panel" : "side bar";
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
  <style>
    body { font-family: var(--vscode-font-family); color: var(--vscode-descriptionForeground); padding: 1rem; }
    p { margin: 0; }
  </style>
</head>
<body>
  <p>No conversation here yet. Right-click a conversation in the Runtrol sidebar and open it in the ${where}.</p>
</body>
</html>`;
}
