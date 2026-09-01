import { existsSync, readFileSync } from "node:fs";

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
const accented = new Map<string, vscode.Uri>();

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

/// The provider's own glyph with one exact project accent embedded in its SVG.
///
/// VS Code does not apply a `ThemeColor` to a custom terminal icon URI. The sidebar and terminal tab therefore use
/// this same immutable data URI, which keeps the provider shape and makes their colour bytes identical. The source
/// SVG is a shipped asset selected through the same closed filename rule as the ordinary icon.
export function accentedConversationIcon(
  extensionUri: vscode.Uri,
  declared: string,
  accent: string,
): vscode.Uri {
  if (!/^#[0-9a-f]{6}$/u.test(accent)) throw new Error("conversation accent must be a lowercase six-digit hex colour");
  const source = conversationIcon(extensionUri, declared);
  const key = `${source.fsPath}\0${accent}`;
  const remembered = accented.get(key);
  if (remembered) return remembered;
  let svg = readFileSync(source.fsPath, "utf8")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gu, "")
    .replace(/\b(fill|stroke)="(?:currentColor|#[0-9a-fA-F]{3,8})"/gu, `$1="${accent}"`);
  if (!/<svg\b[^>]*\bfill=/u.test(svg)) svg = svg.replace(/<svg\b/u, `<svg fill="${accent}"`);
  const answer = vscode.Uri.parse(`data:image/svg+xml;base64,${Buffer.from(svg, "utf8").toString("base64")}`);
  accented.set(key, answer);
  return answer;
}
