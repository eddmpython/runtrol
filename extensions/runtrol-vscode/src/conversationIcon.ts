import { existsSync } from "node:fs";

import * as vscode from "vscode";

/// The coding-service glyph placed on an individual conversation tab.
///
/// Build output is derived from provider manifests, so this projection contains no provider table. A malformed
/// runtime value gets the neutral coding-service glyph and can never escape the resources directory.
export function conversationIcon(extensionUri: vscode.Uri, declared: string): vscode.Uri {
  const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle";
  const candidate = vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`);
  return existsSync(candidate.fsPath)
    ? candidate
    : vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg");
}
