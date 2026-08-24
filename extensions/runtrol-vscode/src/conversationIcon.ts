import { existsSync } from "node:fs";

import * as vscode from "vscode";

/// The coding-service glyph, for a conversation tab and for a sidebar row.
///
/// Build output is derived from provider manifests, so this projection contains no provider table. A malformed
/// runtime value gets the neutral coding-service glyph and can never escape the resources directory.
///
/// The answer is remembered per service name. A tab asks once, but the sidebar asks for every row it draws and
/// redraws every row whenever anything about the list changes, so without this a selection would spend one
/// synchronous disk check per visible conversation. The shipped folder cannot change while the window runs: it
/// is written at build time and read from the installed extension.
const resolved = new Map<string, vscode.Uri>();

export function conversationIcon(extensionUri: vscode.Uri, declared: string): vscode.Uri {
  const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle";
  const remembered = resolved.get(icon);
  if (remembered) return remembered;
  const candidate = vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`);
  const answer = existsSync(candidate.fsPath)
    ? candidate
    : vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg");
  resolved.set(icon, answer);
  return answer;
}
