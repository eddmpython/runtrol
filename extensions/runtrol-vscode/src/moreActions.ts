/// The rare actions, behind the one `⋮` in the sidebar's title bar.
///
/// # Why they are not on a row
///
/// They used to live in a strip the page drew under the title bar: a whole row of the panel holding one button.
/// The operator asked why a second row exists when the first one is right there (2026-08-28), and the answer was
/// that the page could draw a `⋮` and the title bar could not. It can: a title-bar command with a codicon is the
/// same glyph, in the header where the other two actions already are.
///
/// # Why a picker rather than a menu
///
/// The page's own menu had to draw itself, keep its own open state, close on Escape and on any click elsewhere,
/// and stay inside a panel that can be 200px wide. A quick pick is the editor's own list: it filters by typing,
/// it is keyboard-reachable without any of that code, and it looks like every other list in VS Code.

import * as vscode from "vscode";

export type MoreAction = {
  readonly command: string;
  readonly label: string;
};

/// One list, in the order a person meets them: what they do often first, machine administration last.
export const MORE_ACTIONS: readonly MoreAction[] = [
  { command: "runtrol.openNextWaiting", label: "Open the next conversation waiting for you" },
  { command: "runtrol.switchSession", label: "Switch conversation..." },
  { command: "runtrol.arrangeConversationGrid", label: "Arrange open conversations in a grid" },
  { command: "runtrol.refresh", label: "Look again" },
  { command: "runtrol.setUpServices", label: "Set up coding services" },
  { command: "runtrol.checkProviderUpdates", label: "Check for service updates" },
  { command: "runtrol.pairPhone", label: "Pair a phone" },
  { command: "runtrol.managePhones", label: "Manage phones" },
  { command: "runtrol.reviewIntegrations", label: "Review Runtime integrations" },
  { command: "runtrol.manageIntegrations", label: "Manage Runtime integrations" },
  { command: "runtrol.reviewRuntimeRequests", label: "Review Runtime requests" },
  { command: "runtrol.restartExtensionHost", label: "Restart the Extension Host" },
];

/// Offer the rare actions and run the one chosen, with the reason the list is incomplete offered first when
/// there is one. Nothing happens when the picker is dismissed, which is what dismissing a picker means.
export async function showMoreActions(incompleteListing: string | null): Promise<void> {
  const actions: readonly MoreAction[] = incompleteListing
    ? [{ command: "runtrol.explainListing", label: "Why is the list incomplete?" }, ...MORE_ACTIONS]
    : MORE_ACTIONS;
  const chosen = await vscode.window.showQuickPick(
    actions.map((action) => ({ label: action.label, command: action.command })),
    { title: "Runtrol", placeHolder: "What would you like to do?" },
  );
  if (!chosen) return;
  await vscode.commands.executeCommand(chosen.command);
}
