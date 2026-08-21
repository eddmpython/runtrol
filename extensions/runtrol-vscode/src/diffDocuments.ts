import * as vscode from "vscode";

import type { DeclaredDiff } from "./webview/toolDiff";

/// The most review sides held for the editor at once: one 1,024-Artifact Mission Landing, two sides.
/// The shared text ceiling below also prevents declared changes from becoming another transcript store.
const MAX_HELD = 2_048;
export const MAX_DIFF_TEXT = 8 * 1_024 * 1_024;

/// A declared change or reviewed Mission Artifact, opened in VS Code's own diff editor instead of drawn in the page.
///
/// Declared `oldText`/`newText` and exact Landing sides become read-only virtual documents that VS Code draws;
/// a unified patch opens as a read-only `.diff` document the editor highlights itself. Runtrol colours
/// nothing and keeps nothing on disk: the texts live in this map, bounded, for as long as the editor may
/// ask for them, and they are the service's own words relayed into the place VS Code reads a change.
export class DiffDocuments implements vscode.TextDocumentContentProvider {
  static readonly scheme = "runtrol-diff";
  private readonly texts = new Map<string, string>();
  private heldChars = 0;
  private serial = 0;

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.texts.get(uri.path) ?? "";
  }

  /// Open the declared change where VS Code shows changes.
  async open(diff: DeclaredDiff): Promise<void> {
    const name = diff.path ? diff.path.split(/[\\/]/u).pop() || "change" : "change";
    if (diff.kind === "oldNew") {
      const left = this.snapshot(diff.oldText, `before/${name}`);
      const right = this.snapshot(diff.newText, `after/${name}`);
      await vscode.commands.executeCommand(
        "vscode.diff",
        left,
        right,
        `${diff.path || "change"} (declared by the coding service)`,
        { preview: true },
      );
      return;
    }
    const patch = this.snapshot(diff.text, `${name}.diff`);
    const document = await vscode.workspace.openTextDocument(patch);
    await vscode.window.showTextDocument(document, { preview: true });
  }

  snapshot(text: string, name: string): vscode.Uri {
    this.serial += 1;
    const path = `/${this.serial}/${name}`;
    this.texts.set(path, text);
    this.heldChars += text.length;
    while (this.texts.size > MAX_HELD || this.heldChars > MAX_DIFF_TEXT) {
      const oldest = this.texts.entries().next().value;
      if (oldest === undefined) break;
      this.heldChars -= oldest[1].length;
      this.texts.delete(oldest[0]);
    }
    return vscode.Uri.from({ scheme: DiffDocuments.scheme, path });
  }
}
