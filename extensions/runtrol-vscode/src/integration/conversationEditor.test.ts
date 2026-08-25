import * as vscode from "vscode";

export function allTabs(): vscode.Tab[] {
  return vscode.window.tabGroups.all.flatMap((group) => group.tabs);
}

/// A conversation is the service's own terminal in an editor tab (docs/terminalSurface.md).
export function isConversationEditor(tab: vscode.Tab): boolean {
  return tab.input instanceof vscode.TabInputTerminal;
}

export function activeConversationEditor(): vscode.Tab | undefined {
  return allTabs().find((tab) => tab.isActive && isConversationEditor(tab));
}

export function conversationTabDiagnostics(): string {
  return JSON.stringify(allTabs().map((tab) => {
    const input = tab.input as { constructor?: { name?: unknown }; viewType?: unknown };
    return {
      active: tab.isActive,
      input: typeof input.constructor?.name === "string" ? input.constructor.name : typeof tab.input,
      label: tab.label,
      viewType: typeof input.viewType === "string" ? input.viewType : null,
    };
  }));
}
