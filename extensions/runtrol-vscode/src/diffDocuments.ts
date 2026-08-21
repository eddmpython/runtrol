import * as vscode from "vscode";

import type { DeclaredDiff } from "./webview/toolDiff";

/// The most declared changes held for the editor at once. A conversation declares a few per turn; holding
/// more than this would be a transcript of changes, which the provider already keeps.
const MAX_HELD = 64;

/// A change a coding service declared, opened in VS Code's own diff editor instead of drawn in the page.
///
/// The service's `oldText`/`newText` become two read-only virtual documents and `vscode.diff` draws them;
/// a unified patch opens as a read-only `.diff` document the editor highlights itself. Runtrol colours
/// nothing and keeps nothing on disk: the texts live in this map, bounded, for as long as the editor may
/// ask for them, and they are the service's own words relayed into the place VS Code reads a change.
export class DiffDocuments implements vscode.TextDocumentContentProvider {
  static readonly scheme = "runtrol-diff";
  private readonly texts = new Map<string, string>();
  private serial = 0;

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.texts.get(uri.path) ?? "";
  }

  /// Open the declared change where VS Code shows changes.
  async open(diff: DeclaredDiff): Promise<void> {
    const name = diff.path ? diff.path.split(/[\\/]/u).pop() || "change" : "change";
    if (diff.kind === "oldNew") {
      const left = this.hold(diff.oldText, `before/${name}`);
      const right = this.hold(diff.newText, `after/${name}`);
      await vscode.commands.executeCommand(
        "vscode.diff",
        left,
        right,
        `${diff.path || "change"} (declared by the coding service)`,
        { preview: true },
      );
      return;
    }
    const patch = this.hold(diff.text, `${name}.diff`);
    const document = await vscode.workspace.openTextDocument(patch);
    await vscode.window.showTextDocument(document, { preview: true });
  }

  private hold(text: string, name: string): vscode.Uri {
    this.serial += 1;
    const path = `/${this.serial}/${name}`;
    this.texts.set(path, text);
    while (this.texts.size > MAX_HELD) {
      const oldest = this.texts.keys().next().value;
      if (oldest === undefined) break;
      this.texts.delete(oldest);
    }
    return vscode.Uri.from({ scheme: DiffDocuments.scheme, path });
  }
}
