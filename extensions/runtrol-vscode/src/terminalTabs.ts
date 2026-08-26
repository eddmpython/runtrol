import {
  PUBLIC_LIMITS,
  newMutationRequestId,
  type TerminalControlLease,
  type TerminalView,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";
import type { Conversation, StartedConversation } from "./conversationList";
import { projectColorId } from "./projectColor";
import type { StudioRuntimeClient } from "./runtimeClient";

/// The conversation surface: the coding service's own terminal interface, hosted by the Core on a pseudo
/// terminal it owns, shown here as an editor-area tab (`docs/terminalSurface.md`).
///
/// One tab per conversation. The tab is a VS Code terminal whose pseudoterminal is a public Runtime
/// connection: what the service draws comes down as bytes and is written to the tab as written; what the
/// person types goes up as bytes. Nothing here reads either. Splitting, grids and full screen are VS
/// Code's own editor-tab behaviour, which is the point of using its terminal rather than a page of ours.
///
/// The Core answers the terminal's own questions and translates the mouse, so two viewers (this tab and
/// a phone) share one screen and one keyboard.
export class TerminalTabs implements vscode.Disposable {
  private readonly open = new Map<string, vscode.Terminal>();
  /// Conversations opened here that no service has described yet, by the tab showing each one.
  private readonly started = new Map<vscode.Terminal, StartedConversation>();
  private nextStarted = 0;
  private readonly closing: vscode.Disposable;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    /// The glyph for a conversation's service, drawn on the tab so two services' tabs tell apart at a glance.
    private readonly iconFor: (conversation: Conversation) => vscode.ThemeIcon | vscode.Uri,
    /// The same glyph by service id, for a fresh conversation that has no row yet.
    private readonly iconForProvider: (providerId: string) => vscode.ThemeIcon | vscode.Uri,
    /// Told whenever the set of not-yet-described conversations changes, so the list redraws at once.
    private readonly startedChanged: () => void = () => undefined,
  ) {
    this.closing = vscode.window.onDidCloseTerminal((terminal) => {
      for (const [key, open] of this.open) {
        if (open === terminal) this.open.delete(key);
      }
      // A tab closed before its service named the conversation takes its placeholder row with it. Leaving the row
      // would leave a conversation on screen that nothing on this machine can open.
      if (this.started.delete(terminal)) this.startedChanged();
    });
  }

  /// Show the conversation's terminal: the tab that already shows it, or a new one beside the active editor.
  show(conversation: Conversation, preserveFocus: boolean): vscode.Terminal {
    const existing = this.open.get(conversation.key);
    if (existing) {
      existing.show(preserveFocus);
      return existing;
    }
    const native = conversation.native?.adoptionToken
      ? {
          nativeSessionId: conversation.native.nativeSessionId,
          adoptionToken: conversation.native.adoptionToken,
        }
      : null;
    const pty = new RuntimeTerminal(this.runtime, {
      provider: conversation.providerId,
      native,
      workspace: conversation.workspace,
      blocked: conversation.session?.nativeSessionId && !native
        ? "This conversation has no current provider resume proof for the public Runtime terminal."
        : null,
    });
    // The tab is named for the conversation and coloured for its project. The name answers "which conversation",
    // the colour answers "whose project", and the two together fit in the width a tab actually has.
    const colour = conversation.projectless ? null : projectColorId(conversation.workspace);
    const terminal = vscode.window.createTerminal({
      name: conversation.title,
      iconPath: this.iconFor(conversation),
      color: colour ? new vscode.ThemeColor(colour) : undefined,
      pty,
      location: vscode.TerminalLocation.Editor,
      isTransient: true,
    });
    this.open.set(conversation.key, terminal);
    terminal.show(preserveFocus);
    return terminal;
  }

  /// Start a fresh conversation with a service in a folder: the service's terminal interface opens with no
  /// conversation to reopen, and the service creates one. The tab is named for the folder until the
  /// service's own listing gives the conversation a title (the sidebar shows it once the store does).
  showFresh(providerId: string, workspace: string, name: string, projectless = false): vscode.Terminal {
    const pty = new RuntimeTerminal(this.runtime, {
      provider: providerId,
      native: null,
      workspace,
      blocked: null,
    });
    const colour = projectless ? null : projectColorId(workspace);
    const terminal = vscode.window.createTerminal({
      name,
      iconPath: this.iconForProvider(providerId),
      color: colour ? new vscode.ThemeColor(colour) : undefined,
      pty,
      location: vscode.TerminalLocation.Editor,
      isTransient: true,
    });
    this.started.set(terminal, {
      id: `${providerId}:${this.nextStarted += 1}`,
      providerId,
      workspace,
      title: name,
      startedAtMs: Date.now(),
    });
    this.startedChanged();
    terminal.show(false);
    return terminal;
  }

  /// Whether this conversation already has its tab open here.
  isOpen(conversationKey: string): boolean {
    return this.open.has(conversationKey);
  }

  /// The conversations this window opened that no service has described yet.
  startedConversations(): StartedConversation[] {
    return [...this.started.values()];
  }

  /// Spread the open conversation tabs over editor groups: each tab after the first moves to a group of
  /// its own, and four or more become the editor's two-by-two grid. Returns how many tabs were arranged.
  async arrangeGrid(): Promise<number> {
    const open = [...this.open.values()];
    if (open.length < 2) return 0;
    await vscode.commands.executeCommand("workbench.action.editorLayoutSingle");
    for (const [index, terminal] of open.entries()) {
      terminal.show(false);
      if (index > 0) await vscode.commands.executeCommand("workbench.action.moveEditorToNewGroup");
    }
    if (open.length >= 4) await vscode.commands.executeCommand("workbench.action.editorLayoutTwoByTwoGrid");
    return open.length;
  }

  dispose(): void {
    this.closing.dispose();
    for (const terminal of this.open.values()) terminal.dispose();
    this.open.clear();
  }
}

type Target = {
  provider: string;
  native: { nativeSessionId: string; adoptionToken: string } | null;
  workspace: string;
  blocked: string | null;
};

/// A VS Code pseudoterminal whose other end is the Core's hosted terminal.
///
/// Output is decoded as streaming UTF-8 (a multi-byte character may straddle two chunks). Input is sent as
/// the UTF-8 bytes of what VS Code hands over, which for mouse reports and special keys is already the
/// terminal's own escape vocabulary.
class RuntimeTerminal implements vscode.Pseudoterminal {
  private readonly writeEmitter = new vscode.EventEmitter<string>();
  private readonly closeEmitter = new vscode.EventEmitter<number | void>();
  readonly onDidWrite = this.writeEmitter.event;
  readonly onDidClose = this.closeEmitter.event;
  private view: TerminalView | null = null;
  private lease: TerminalControlLease | null = null;
  private decoder = new TextDecoder("utf-8");
  private closed = false;
  private commandTail = Promise.resolve();
  private dimensions = { columns: 120, rows: 40 };
  /// Input typed before the connection is up is kept and sent once it is: a person who starts typing while
  /// the tab opens must not lose the first keys.
  private pending: Uint8Array[] = [];
  private pendingBytes = 0;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    private readonly target: Target,
  ) {}

  /// Every failure below leaves the tab standing with the reason written in it.
  ///
  /// Closing it wrote the one sentence that explains the failure and took it away in the same frame, so a
  /// conversation that would not open left no trace at all: a tab flashed and the person was back where they
  /// started with nothing to read (measured 2026-08-26).
  open(initialDimensions: vscode.TerminalDimensions | undefined): void {
    this.dimensions = {
      columns: initialDimensions?.columns ?? 120,
      rows: initialDimensions?.rows ?? 40,
    };
    void this.connect().catch((error: unknown) => this.fail(error));
  }

  close(): void {
    this.detach(true);
  }

  handleInput(data: string): void {
    if (this.closed) return;
    const bytes = Buffer.from(data, "utf8");
    if (!this.view) {
      if (this.pendingBytes + bytes.byteLength > PUBLIC_LIMITS.maxTerminalWriteBytes) {
        this.fail(new Error("Input entered while the terminal opened exceeded the Runtime input bound."));
        return;
      }
      this.pending.push(bytes);
      this.pendingBytes += bytes.byteLength;
      return;
    }
    this.queueControl(async (view, lease) => {
      await view.write({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        bytesBase64: Buffer.from(bytes).toString("base64"),
      });
    });
  }

  setDimensions(dimensions: vscode.TerminalDimensions): void {
    this.dimensions = { columns: dimensions.columns, rows: dimensions.rows };
    if (this.closed || !this.view) return;
    this.queueControl(async (view, lease) => {
      await view.resize({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        geometry: this.dimensions,
      });
    });
  }

  private async connect(): Promise<void> {
    if (this.target.blocked) throw new Error(this.target.blocked);
    const geometry = this.dimensions;
    const view = await this.runtime.openTerminal({
      requestId: newMutationRequestId(),
      providerId: this.target.provider,
      workspace: this.target.workspace,
      target: this.target.native
        ? { kind: "native", ...this.target.native }
        : { kind: "fresh" },
      geometry,
    });
    if (this.closed) {
      view.close();
      return;
    }
    this.view = view;
    this.lease = view.opened.controlLease ?? null;
    this.writeEmitter.fire(this.decoder.decode(view.initialScreen, { stream: true }));
    if (
      this.dimensions.columns !== geometry.columns
      || this.dimensions.rows !== geometry.rows
    ) {
      this.setDimensions(this.dimensions);
    }
    const pending = this.pending;
    this.pending = [];
    this.pendingBytes = 0;
    for (const bytes of pending) {
      this.queueControl(async (opened, lease) => {
        await opened.write({
          requestId: newMutationRequestId(),
          terminalId: opened.opened.terminal.terminalId,
          leaseId: lease.leaseId,
          leaseGeneration: lease.leaseGeneration,
          bytesBase64: Buffer.from(bytes).toString("base64"),
        });
      });
    }
    await this.pump(view);
  }

  /// Read the view until the service ends or the connection does.
  private async pump(view: TerminalView): Promise<void> {
    try {
      for (;;) {
        const notification = await view.next();
        switch (notification.kind) {
          case "output":
            this.writeEmitter.fire(this.decoder.decode(notification.bytes, { stream: true }));
            break;
          case "lagged":
            // The Core re-sends the whole screen next; clear so the redraw lands on a clean page, and start
            // decoding afresh so a multibyte tail cut off by the lag never bleeds into it.
            this.decoder = new TextDecoder("utf-8");
            this.writeEmitter.fire("\x1b[2J\x1b[H");
            this.writeEmitter.fire(this.decoder.decode(notification.screen, { stream: true }));
            break;
          case "exited":
            // A clean exit closes the tab like a shell's would. Anything else keeps the tab, with the
            // service's own last words on it: a resume the service refused (measured: an empty stored
            // conversation exits at once) must not vanish before the person can read why.
            if (notification.exitCode === 0) {
              this.end(0);
            } else {
              this.writeEmitter.fire(`
\x1b[2m[${this.target.provider} ended with code ${notification.exitCode}]\x1b[0m
`);
              this.detach(false);
            }
            return;
        }
      }
    } catch (error) {
      if (this.closed) return;
      this.fail(error);
    }
  }

  private queueControl(
    action: (view: TerminalView, lease: TerminalControlLease) => Promise<void>,
  ): void {
    const command = this.commandTail.then(async () => {
      if (this.closed) return;
      const view = this.view;
      if (!view) throw new Error("The public Runtime terminal is not connected.");
      await action(view, await this.ensureControl(view));
    });
    this.commandTail = command.then(
      () => undefined,
      () => undefined,
    );
    void command.catch((error: unknown) => this.fail(error));
  }

  private async ensureControl(view: TerminalView): Promise<TerminalControlLease> {
    const lease = this.lease;
    if (lease && lease.expiresAtMs > Date.now() + 5_000) return lease;
    this.lease = lease
      ? await view.renewControl({
          requestId: newMutationRequestId(),
          terminalId: view.opened.terminal.terminalId,
          leaseId: lease.leaseId,
          leaseGeneration: lease.leaseGeneration,
        })
      : await view.acquireControl({
          requestId: newMutationRequestId(),
          terminalId: view.opened.terminal.terminalId,
          expectedTerminalGeneration: view.opened.terminal.terminalGeneration,
        });
    return this.lease;
  }

  private fail(error: unknown): void {
    if (this.closed) return;
    const message = error instanceof Error ? error.message : String(error);
    this.writeEmitter.fire(`\r\n\x1b[31m${message}\x1b[0m\r\n`);
    this.detach(false);
  }

  private end(code?: number): void {
    if (this.closed) return;
    this.detach(false);
    this.closeEmitter.fire(code);
  }

  /// Stop carrying the view but leave the tab open, so what the service wrote last stays readable.
  private detach(notifyRuntime: boolean): void {
    if (this.closed) return;
    this.closed = true;
    const view = this.view;
    this.view = null;
    this.lease = null;
    if (!view) return;
    if (notifyRuntime) {
      void view.detach({
        terminalId: view.opened.terminal.terminalId,
        viewId: view.opened.viewId,
      }).catch(() => view.close());
    } else {
      view.close();
    }
  }
}
