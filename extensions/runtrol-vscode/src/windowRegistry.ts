import * as vscode from "vscode";
import type { WindowRegisterParams, WindowUpdateParams } from "@runtrol/runtime-client";

import { WindowRegistryState } from "./windowRegistryState";

/// Where the window's record goes.
export type WindowPublisher = {
  publishWindow(register: WindowRegisterParams, update: WindowUpdateParams): Promise<void>;
};

/// This window's entry in the Runtime's window registry, kept current by VS Code's own terminal events and
/// nothing else: a terminal opened or closed, shell integration attached, a command started or ended, the
/// workspace folders changed. One publish is in flight at a time and a change that lands meanwhile is sent right
/// after it, so the registry always ends on the latest state without a queue growing behind a slow Runtime.
export class WindowRegistry implements vscode.Disposable {
  private readonly state: WindowRegistryState;
  private readonly subscriptions: vscode.Disposable[] = [];
  private inFlight = false;
  private dirty = false;
  private disposed = false;
  private lastReported: string | null = null;

  constructor(
    private readonly publisher: WindowPublisher,
    private readonly report: (message: string) => void,
  ) {
    this.state = new WindowRegistryState(
      {
        windowSessionId: vscode.env.sessionId,
        hostGeneration: `${process.pid}-${Date.now()}`,
        vscodeVersion: vscode.version,
      },
      folders(),
    );
  }

  /// Register now, with every terminal the window already holds, and follow the events from here on.
  start(): void {
    for (const terminal of vscode.window.terminals) this.track(terminal);
    this.subscriptions.push(
      vscode.window.onDidOpenTerminal((terminal) => {
        this.track(terminal);
        this.schedule();
      }),
      vscode.window.onDidCloseTerminal((terminal) => {
        if (this.state.closed(terminal)) this.schedule();
      }),
      vscode.window.onDidChangeTerminalShellIntegration((change) => {
        if (this.state.shellIntegrationChanged(change.terminal, change.shellIntegration.cwd?.fsPath ?? null)) {
          this.schedule();
        }
      }),
      vscode.window.onDidStartTerminalShellExecution((start) => {
        const commandLine = start.execution.commandLine;
        if (this.state.executionStarted(start.terminal, commandLine.value, commandLine.confidence, Date.now()) !== null) {
          this.schedule();
        }
      }),
      vscode.window.onDidEndTerminalShellExecution((end) => {
        if (this.state.executionEnded(end.terminal)) this.schedule();
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        this.state.foldersChanged(folders());
        this.schedule();
      }),
    );
    this.schedule();
  }

  dispose(): void {
    this.disposed = true;
    for (const subscription of this.subscriptions) subscription.dispose();
    this.subscriptions.length = 0;
  }

  private track(terminal: vscode.Terminal): void {
    this.state.opened(terminal, terminal.name);
    if (terminal.shellIntegration) {
      this.state.shellIntegrationChanged(terminal, terminal.shellIntegration.cwd?.fsPath ?? null);
    }
    void terminal.processId.then((processId) => {
      if (this.state.processResolved(terminal, processId)) this.schedule();
    });
  }

  private schedule(): void {
    if (this.disposed) return;
    if (this.inFlight) {
      this.dirty = true;
      return;
    }
    this.inFlight = true;
    void (async () => {
      try {
        do {
          this.dirty = false;
          // Names settle after the shell starts and VS Code raises no event for that; read them at publish time.
          for (const terminal of vscode.window.terminals) this.state.renamed(terminal, terminal.name);
          await this.publisher.publishWindow(this.state.register(), this.state.update());
        } while (this.dirty && !this.disposed);
      } catch (error) {
        // The Runtime is not there or refused the record; the next event tries again. Nothing here may throw
        // into a VS Code event handler, and the same failure is reported once, not on every event.
        const message = error instanceof Error ? error.message : String(error);
        if (message !== this.lastReported) {
          this.lastReported = message;
          this.report(message);
        }
      } finally {
        this.inFlight = false;
      }
    })();
  }
}

function folders(): string[] {
  return (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
}
