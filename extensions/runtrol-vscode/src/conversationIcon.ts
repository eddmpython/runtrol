import { existsSync, readFileSync } from "node:fs";
import { basename } from "node:path";

import * as vscode from "vscode";
import { accentGlyph } from "./conversationGlyph";

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
const terminalIcons = new Map<string, vscode.ThemeIcon>();
let terminalIconIds: Record<string, Record<string, string>> | undefined;

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
/// The sidebar uses this immutable data URI. Its exact accent and provider outline also feed the registered
/// terminal color glyph. The source SVG uses the same closed filename rule as the ordinary icon.
export function accentedConversationIcon(
  extensionUri: vscode.Uri,
  declared: string,
  accent: string,
): vscode.Uri {
  const source = conversationIcon(extensionUri, declared);
  const key = `${source.fsPath}\0${accent}`;
  const remembered = accented.get(key);
  if (remembered) return remembered;
  const svg = accentGlyph(readFileSync(source.fsPath, "utf8"), accent);
  const answer = vscode.Uri.parse(`data:image/svg+xml;base64,${Buffer.from(svg, "utf8").toString("base64")}`);
  accented.set(key, answer);
  return answer;
}

/// Registered color glyphs survive native editor moves because their font belongs to the global icon registry.
/// The build derives both the palette and outline from the same sources as the sidebar SVG.
export function terminalConversationIcon(
  extensionUri: vscode.Uri,
  declared: string,
  accent: string,
): vscode.ThemeIcon {
  const source = conversationIcon(extensionUri, declared);
  const icon = basename(source.fsPath, ".svg");
  const key = `${icon}\0${accent}`;
  const remembered = terminalIcons.get(key);
  if (remembered) return remembered;
  terminalIconIds ??= JSON.parse(readFileSync(
    vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "iconMap.json").fsPath,
    "utf8",
  )) as Record<string, Record<string, string>>;
  const id = terminalIconIds[icon]?.[accent];
  if (typeof id !== "string" || !/^runtrol-[a-z0-9-]+-[0-9a-f]{6}$/u.test(id)) {
    throw new Error("The installed terminal glyph does not match its project accent.");
  }
  const answer = new vscode.ThemeIcon(id);
  terminalIcons.set(key, answer);
  return answer;
}
