import * as vscode from "vscode";

import type { ProviderLine, ProviderUsageList } from "./runtimeTypes";
import { usageRows, type UsageRow } from "./usageDisplay";

/// The usage strip: each account's own latest word on where it stands, at the bottom of the panel.
///
/// Collapsed by default and absent of invention: a provider that has not reported is not listed, and the view's
/// empty text says "no report yet" rather than drawing a green light nobody earned.
///
/// # Why it refreshes on the session list's own changes rather than on a clock
///
/// This extension forbids polling loops outright, and the prohibition is machine-checked. It also happens to be
/// the honest signal here: a gauge moves when a provider reports, a provider reports around turns, and turns
/// starting and ending are exactly what changes the session list. So the strip re-asks when that list changes and
/// when it becomes visible, and each ask is one bounded read of a snapshot the daemon already holds.
export class UsageTree implements vscode.TreeDataProvider<UsageItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<UsageItem | undefined>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private rows: UsageRow[] = [];
  private view: vscode.TreeView<UsageItem> | null = null;
  private fetching = false;
  private behind = false;

  constructor(
    private readonly ports: {
      usage: () => Promise<ProviderUsageList>;
      providers: () => readonly ProviderLine[];
      now: () => number;
    },
  ) {}

  bindView(view: vscode.TreeView<UsageItem>): void {
    this.view = view;
    view.onDidChangeVisibility((event) => {
      if (event.visible) void this.refreshQuietly();
    });
    if (view.visible) void this.refreshQuietly();
  }

  /// The session list changed, which is when a gauge is most likely to have moved. Costs nothing while the
  /// strip is hidden: an invisible view redraws on its next visibility change anyway.
  sessionsChanged(): void {
    if (this.view?.visible) void this.refreshQuietly();
  }

  /// Re-ask the Runtime and redraw. Failures leave the previous rows standing rather than blanking the strip:
  /// a stale number labelled with its age beats an empty box during a reconnect.
  async refresh(): Promise<void> {
    if (this.fetching) {
      // One read at a time, but a change that arrives mid-read is not dropped: the read that is running answers
      // the question that was current when it started, so the strip asks once more when it finishes.
      this.behind = true;
      return;
    }
    this.fetching = true;
    try {
      const usage = await this.ports.usage();
      this.rows = usageRows(usage.providers, this.ports.providers(), this.ports.now());
      if (this.view) {
        this.view.message = this.rows.length === 0 ? "No account has reported yet." : undefined;
      }
      this.changedEmitter.fire(undefined);
    } finally {
      this.fetching = false;
      if (this.behind) {
        this.behind = false;
        void this.refreshQuietly();
      }
    }
  }

  /// A refresh whose failure leaves the previous rows standing rather than surfacing: a stale number whose hover
  /// already says its age beats an error box for a read that the next session change will retry anyway.
  private async refreshQuietly(): Promise<void> {
    try {
      await this.refresh();
    } catch {
      // Deliberate: see above. The rows keep their reported-at ages and the next change asks again.
    }
  }

  getTreeItem(element: UsageItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: UsageItem): UsageItem[] {
    if (element) return [];
    return this.rows.map((row) => new UsageItem(row));
  }

  dispose(): void {
    this.changedEmitter.dispose();
  }
}

/// One account's line.
export class UsageItem extends vscode.TreeItem {
  constructor(row: UsageRow) {
    super(row.name, vscode.TreeItemCollapsibleState.None);
    this.id = row.key;
    this.description = row.detail;
    this.tooltip = row.tooltip;
    this.contextValue = "runtrol.usage";
    this.iconPath = new vscode.ThemeIcon(
      row.icon,
      row.reached ? new vscode.ThemeColor("problemsErrorIcon.foreground") : undefined,
    );
  }
}
