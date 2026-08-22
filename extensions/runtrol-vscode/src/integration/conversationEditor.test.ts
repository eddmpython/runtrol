import * as vscode from "vscode";

const CONVERSATION_VIEW_TYPES = new Set([
  "runtrol.conversation",
  "mainThreadWebview-runtrol.conversation",
]);

export function allTabs(): vscode.Tab[] {
  return vscode.window.tabGroups.all.flatMap((group) => group.tabs);
}

export function isConversationEditor(tab: vscode.Tab): boolean {
  return tab.input instanceof vscode.TabInputWebview
    && CONVERSATION_VIEW_TYPES.has(tab.input.viewType);
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
