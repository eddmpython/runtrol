import * as vscode from "vscode";

import type { ProviderLine, SessionLine, WatchCursor } from "./protocol";
import { providerRowsEqual, sessionRowsEqual } from "./stateRows";

export class RuntimeState implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  private readonly cursors = new Map<string, WatchCursor>();
  private sessionRows: readonly SessionLine[] = [];
  private providerRows: readonly ProviderLine[] = [];
  private selectedId: string | null = null;

  readonly onDidChange = this.changedEmitter.event;

  get sessions(): readonly SessionLine[] {
    return this.sessionRows;
  }

  get providers(): readonly ProviderLine[] {
    return this.providerRows;
  }

  get selected(): SessionLine | null {
    return this.sessionRows.find((session) => session.session === this.selectedId) ?? null;
  }

  replace(sessions: readonly SessionLine[], providers: readonly ProviderLine[]): void {
    if (sessionRowsEqual(this.sessionRows, sessions) && providerRowsEqual(this.providerRows, providers)) {
      return;
    }
    this.sessionRows = sessions;
    this.providerRows = providers;
    if (this.selectedId && !sessions.some((session) => session.session === this.selectedId)) {
      this.selectedId = null;
    }
    this.changedEmitter.fire();
  }

  select(session: string | null): void {
    if (this.selectedId === session) {
      return;
    }
    this.selectedId = session;
    this.changedEmitter.fire();
  }

  cursor(session: string): WatchCursor | null {
    return this.cursors.get(session) ?? null;
  }

  advance(session: string, cursor: WatchCursor): void {
    this.cursors.set(session, cursor);
  }

  dispose(): void {
    this.cursors.clear();
    this.changedEmitter.dispose();
  }
}
