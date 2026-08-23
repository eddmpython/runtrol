import * as vscode from "vscode";

import type { ProviderLine, ProviderUsageList } from "./runtimeTypes";
import { usageRows, type UsageRow } from "./usageDisplay";

/// Every connected CLI's usage, kept open at the bottom of the Runtrol sidebar.
///
/// It refreshes from bounded Runtime snapshots when the session index changes. There is no timer and no hidden
/// second page. A CLI that has not reported remains present and says so instead of being omitted or shown green.
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
    this.rows = usageRows([], this.ports.providers(), this.ports.now());
    this.changedEmitter.fire(undefined);
    view.onDidChangeVisibility((event) => {
      if (event.visible) void this.refreshQuietly();
    });
    if (view.visible) void this.refreshQuietly();
  }

  sessionsChanged(): void {
    if (this.view?.visible) void this.refreshQuietly();
  }

  async refresh(): Promise<void> {
    if (this.fetching) {
      this.behind = true;
      return;
    }
    this.fetching = true;
    try {
      const usage = await this.ports.usage();
      this.rows = usageRows(usage.providers, this.ports.providers(), this.ports.now());
      if (this.view) {
        this.view.message = this.rows.length === 0 ? "No connected CLI." : undefined;
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

  private async refreshQuietly(): Promise<void> {
    try {
      await this.refresh();
    } catch {
      // Deliberate: previous reports remain visible and the next session or visibility event retries.
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
