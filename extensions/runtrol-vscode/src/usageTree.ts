import * as vscode from "vscode";

import type { ProviderLine, ProviderUsageGauge, ProviderUsageList } from "./runtimeTypes";
import { usageRows, usageRowsEqual, type UsageRow } from "./usageDisplay";

/// Every installed CLI's operational state and usage, kept open at the bottom of the Runtrol sidebar.
///
/// It refreshes from bounded Runtime snapshots when the session index changes. There is no timer and no hidden
/// second page. A CLI that has not reported remains present and says so instead of being omitted or shown green.
export class UsageTree implements vscode.TreeDataProvider<UsageItem>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<UsageItem | undefined>();
  readonly onDidChangeTreeData = this.changedEmitter.event;
  private gauges: readonly ProviderUsageGauge[] = [];
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
    this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()), true);
    view.onDidChangeVisibility((event) => {
      if (event.visible) void this.refreshQuietly();
    });
    if (view.visible) void this.refreshQuietly();
  }

  sessionsChanged(): void {
    // Provider availability is already in memory. Draw that state immediately instead of waiting behind the usage
    // request, while retaining the last explicitly aged gauge until a fresh snapshot arrives.
    this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()));
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
      this.gauges = usage.providers;
      this.publish(usageRows(this.gauges, this.ports.providers(), this.ports.now()));
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
      // Previous reports remain visible, but the failure must not look like a successful current snapshot.
      if (this.view) this.view.message = "Usage refresh failed. Showing the last report.";
    }
  }

  private publish(next: UsageRow[], force = false): void {
    const changed = !usageRowsEqual(this.rows, next);
    this.rows = next;
    if (this.view) this.view.message = this.rows.length === 0 ? "No connected CLI." : undefined;
    if (changed || force) this.changedEmitter.fire(undefined);
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
  readonly provider: ProviderLine | null;

  constructor(row: UsageRow) {
    super(row.name, vscode.TreeItemCollapsibleState.None);
    this.provider = row.provider;
    this.id = row.key;
    this.description = row.detail;
    this.tooltip = row.tooltip;
    this.contextValue = row.state === "unavailable" ? "runtrol.usageProblem" : "runtrol.usage";
    this.iconPath = new vscode.ThemeIcon(
      row.icon,
      row.reached
        ? new vscode.ThemeColor("problemsErrorIcon.foreground")
        : row.state === "unavailable"
          ? new vscode.ThemeColor("problemsWarningIcon.foreground")
          : row.state === "checking" || row.state === "disconnected"
            ? new vscode.ThemeColor("descriptionForeground")
            : undefined,
    );
    if (row.state === "unavailable") {
      this.command = {
        command: "runtrol.fixService",
        title: "Fix coding service",
        arguments: [this],
      };
    }
    this.accessibilityInformation = {
      label: row.state === "unavailable"
        ? `${row.name}, unavailable, fixes available`
        : `${row.name}, ${row.detail}`,
    };
  }
}
