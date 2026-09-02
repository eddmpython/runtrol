import type {
  ObservedCommand,
  ObservedTerminal,
  WindowRegisterParams,
  WindowUpdateParams,
} from "@runtrol/runtime-client";

/// What this window publishes to the Runtime's window registry, kept as a pure record of the terminal events VS
/// Code delivered so the projection can be tested without an Extension Host (`docs/vscodeSurface.md`, window
/// registry). The window's identity is VS Code's session id (kept across an Extension Host restart, renewed by a
/// reload; measured 2026-09-02); the host generation is minted once per activation so a restarted host is told
/// apart from the one before it.
export type WindowIdentity = {
  readonly windowSessionId: string;
  readonly hostGeneration: string;
  readonly vscodeVersion: string;
};

type Observed = {
  readonly key: string;
  name: string;
  processId: number | null;
  shellIntegration: boolean;
  cwd: string | null;
  command: ObservedCommand | null;
};

export class WindowRegistryState {
  private readonly terminals = new Map<object, Observed>();
  private nextTerminal = 0;
  private nextExecution = 0;
  private folders: readonly string[];

  constructor(private readonly identity: WindowIdentity, folders: readonly string[]) {
    this.folders = [...folders];
  }

  /// A terminal VS Code lists or opened. The handle is whatever object VS Code uses for it; it is never read.
  opened(handle: object, name: string): void {
    if (this.terminals.has(handle)) return;
    this.nextTerminal += 1;
    this.terminals.set(handle, {
      key: `t${this.nextTerminal}`,
      name,
      processId: null,
      shellIntegration: false,
      cwd: null,
      command: null,
    });
  }

  /// VS Code names a terminal after its shell started, so the name is read again before each publish.
  renamed(handle: object, name: string): boolean {
    const terminal = this.terminals.get(handle);
    if (!terminal || terminal.name === name) return false;
    terminal.name = name;
    return true;
  }

  closed(handle: object): boolean {
    return this.terminals.delete(handle);
  }

  processResolved(handle: object, processId: number | undefined): boolean {
    const terminal = this.terminals.get(handle);
    if (!terminal || processId === undefined || terminal.processId === processId) return false;
    terminal.processId = processId;
    return true;
  }

  shellIntegrationChanged(handle: object, cwd: string | null): boolean {
    const terminal = this.terminals.get(handle);
    if (!terminal) return false;
    terminal.shellIntegration = true;
    terminal.cwd = cwd;
    return true;
  }

  /// A command started: the next command generation of this window.
  executionStarted(handle: object, commandLine: string, confidence: number, startedAtMs: number): string | null {
    const terminal = this.terminals.get(handle);
    if (!terminal) return null;
    this.nextExecution += 1;
    const executionId = `e${this.nextExecution}`;
    terminal.command = {
      executionId,
      commandLine: commandLine.slice(0, 1024),
      confidence: Math.max(0, Math.min(2, Math.trunc(confidence))),
      startedAtMs,
    };
    return executionId;
  }

  executionEnded(handle: object): boolean {
    const terminal = this.terminals.get(handle);
    if (!terminal || terminal.command === null) return false;
    terminal.command = null;
    return true;
  }

  foldersChanged(folders: readonly string[]): void {
    this.folders = [...folders];
  }

  register(): WindowRegisterParams {
    return {
      windowSessionId: this.identity.windowSessionId,
      hostGeneration: this.identity.hostGeneration,
      vscodeVersion: this.identity.vscodeVersion,
      workspaceFolders: this.folders.slice(0, 32),
    };
  }

  update(): WindowUpdateParams {
    const terminals: ObservedTerminal[] = [];
    for (const terminal of this.terminals.values()) {
      if (terminals.length >= 64) break;
      terminals.push({
        terminalKey: terminal.key,
        name: terminal.name.slice(0, 1024),
        ...(terminal.processId === null ? {} : { processId: terminal.processId }),
        shellIntegration: terminal.shellIntegration,
        ...(terminal.cwd === null ? {} : { cwd: terminal.cwd }),
        ...(terminal.command === null ? {} : { command: terminal.command }),
      });
    }
    return { terminals };
  }
}
