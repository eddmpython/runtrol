import { randomUUID } from "node:crypto";
import * as vscode from "vscode";
import type { StudioRuntimeClient } from "./runtimeClient";

type Pending = { token: string; execution?: vscode.TaskExecution; ended?: vscode.TaskExecution };
let actions: ProviderAccountActions | null = null;

/// Account execution has no activation work. Its listener is installed on the first explicit command.
export function runAccountCommand(
  context: vscode.ExtensionContext, runtime: StudioRuntimeClient,
  ...args: Parameters<ProviderAccountActions["run"]>
): Promise<void> {
  if (!actions) {
    actions = new ProviderAccountActions(() => {
      void runtime.providersUsage().catch((error: unknown) => {
        void vscode.window.showWarningMessage(`Cannot refresh usage: ${error instanceof Error ? error.message : String(error)}`);
      });
    });
    context.subscriptions.push(actions);
  }
  return actions.run(...args);
}

/// The CLI owns authentication and its terminal. Only the editor's task completion wakes structured usage.
/// Tasks provide a real completion even when the user's shell has no shell integration.
export class ProviderAccountActions implements vscode.Disposable {
  private readonly pending = new Map<string, Pending>();
  private readonly ended: vscode.Disposable;

  constructor(private readonly refresh: () => void) {
    const completed = ({ execution }: vscode.TaskEndEvent): void => {
      const { provider, token } = execution.task.definition;
      const pending = this.pending.get(provider);
      if (!pending || pending.token !== token) return;
      pending.ended = execution;
      this.complete(provider, pending);
    };
    const processEnd = vscode.tasks.onDidEndTaskProcess(completed);
    // A task cancelled before it starts a process must also release its pending action for retry.
    const taskEnd = vscode.tasks.onDidEndTask(completed);
    this.ended = { dispose: () => { processEnd.dispose(); taskEnd.dispose(); } };
  }

  async run(provider: string, version: string | null | undefined, label: string, command: string): Promise<void> {
    if (this.pending.has(provider)) return;
    const pending: Pending = { token: randomUUID() };
    this.pending.set(provider, pending);
    const task = new vscode.Task(
      { type: "runtrol-account", provider, version: version ?? "", token: pending.token }, vscode.TaskScope.Global,
      label, "Runtrol", new vscode.ShellExecution(command), [],
    );
    task.presentationOptions = { reveal: vscode.TaskRevealKind.Always, panel: vscode.TaskPanelKind.Shared,
      focus: true, showReuseMessage: false };
    try {
      pending.execution = await vscode.tasks.executeTask(task);
      this.complete(provider, pending);
    } catch (error) {
      this.pending.delete(provider);
      throw error;
    }
  }

  private complete(provider: string, pending: Pending): void {
    if (!pending.execution || pending.ended !== pending.execution || this.pending.get(provider) !== pending) return;
    this.pending.delete(provider);
    // Exit is a reason to ask, never a verdict about login. The Core prepares the current binary afresh.
    this.refresh();
  }

  dispose(): void {
    this.ended.dispose();
    this.pending.clear();
    if (actions === this) actions = null;
  }
}
