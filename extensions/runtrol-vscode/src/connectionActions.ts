import * as QRCode from "qrcode/lib/core/qrcode";
import * as SvgRenderer from "qrcode/lib/renderer/svg-tag";
import * as vscode from "vscode";
import type { TerminalTabs } from "./terminalTabs";

export { pairPhone, managePhones, reviewPhonePairings } from "./pairingAdministration";
export { runAccountCommand } from "./providerAccountAction";

export async function restartExtensionHost(terminals: TerminalTabs): Promise<void> {
  const confirmed = await vscode.window.showWarningMessage(
    "Restart the VS Code Extension Host? Other extensions in this window will restart too.",
    { modal: true }, "Restart extensions",
  );
  if (confirmed !== "Restart extensions") return;
  await terminals.restartHost(() => vscode.commands.executeCommand("workbench.action.restartExtensionHost"));
}

export function render(value: string): string {
  return SvgRenderer.render(QRCode.create(value));
}
