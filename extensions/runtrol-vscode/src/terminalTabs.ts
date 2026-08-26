import * as vscode from "vscode";
import type { Conversation, StartedConversation } from "./conversationList";
import { projectColorId } from "./projectColor";
import { FrameTransport } from "./core/framing";
import type { CoreLocator } from "./core/locator";
import { type Request, readResponse, requestHello } from "./protocol";

/// The conversation surface: the coding service's own terminal interface, hosted by the Core on a pseudo
/// terminal it owns, shown here as an editor-area tab (`docs/terminalSurface.md`).
///
/// One tab per conversation. The tab is a VS Code terminal whose pseudoterminal is a private-wire
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
    private readonly locator: CoreLocator,
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
    const pty = new CoreTerminal(this.locator, {
      provider: conversation.providerId,
      native: conversation.native?.nativeSessionId ?? conversation.session?.nativeSessionId ?? null,
      workspace: conversation.workspace,
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
    const pty = new CoreTerminal(this.locator, { provider: providerId, native: null, workspace });
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
  native: string | null;
  workspace: string;
};

/// A VS Code pseudoterminal whose other end is the Core's hosted terminal.
///
/// Output is decoded as streaming UTF-8 (a multi-byte character may straddle two chunks). Input is sent as
/// the UTF-8 bytes of what VS Code hands over, which for mouse reports and special keys is already the
/// terminal's own escape vocabulary.
class CoreTerminal implements vscode.Pseudoterminal {
  private readonly writeEmitter = new vscode.EventEmitter<string>();
  private readonly closeEmitter = new vscode.EventEmitter<number | void>();
  readonly onDidWrite = this.writeEmitter.event;
  readonly onDidClose = this.closeEmitter.event;
  private transport: FrameTransport | null = null;
  private decoder = new TextDecoder("utf-8");
  private closed = false;
  /// Input typed before the connection is up is kept and sent once it is: a person who starts typing while
  /// the tab opens must not lose the first keys.
  private pending: string[] = [];

  constructor(
    private readonly locator: CoreLocator,
    private readonly target: Target,
  ) {}

  /// Every failure below leaves the tab standing with the reason written in it.
  ///
  /// Closing it wrote the one sentence that explains the failure and took it away in the same frame, so a
  /// conversation that would not open left no trace at all: a tab flashed and the person was back where they
  /// started with nothing to read (measured 2026-08-26).
  open(initialDimensions: vscode.TerminalDimensions | undefined): void {
    const cols = initialDimensions?.columns ?? 120;
    const rows = initialDimensions?.rows ?? 40;
    void this.connect(cols, rows).catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      this.writeEmitter.fire(`\r\n\x1b[31m${message}\x1b[0m\r\n`);
      this.detach();
    });
  }

  close(): void {
    this.end();
  }

  handleInput(data: string): void {
    if (this.closed) return;
    if (!this.transport) {
      this.pending.push(data);
      return;
    }
    void this.send({ ask: "terminalInput", with: { bytes: Buffer.from(data, "utf8").toString("base64") } });
  }

  setDimensions(dimensions: vscode.TerminalDimensions): void {
    if (this.closed || !this.transport) return;
    void this.send({ ask: "terminalResize", with: { cols: dimensions.columns, rows: dimensions.rows } });
  }

  /// What a draining generation answers when it refuses new work. Matched on the Core's own words because
  /// that refusal is the one failure a viewer can recover from by itself.
  private static readonly DRAINING = "generation is draining";

  private async connect(cols: number, rows: number, attempt = 0): Promise<void> {
    const located = await this.locator.locate();
    const transport = await FrameTransport.connect(located.endpoint);
    try {
      await transport.send(requestHello());
      const welcome = readResponse(JSON.parse((await transport.receive()).toString("utf8")));
      if (welcome.say === "failed") throw new Error(welcome.with.message);
      if (welcome.say !== "welcome") throw new Error(`the Core greeted with ${welcome.say}`);
      const open: Request = {
        ask: "terminalOpen",
        with: { provider: this.target.provider, native: this.target.native, workspace: this.target.workspace, cols, rows },
      };
      await transport.send(open);
      const opened = readResponse(JSON.parse((await transport.receive()).toString("utf8")));
      if (opened.say === "failed") {
        // A generation that has begun draining refuses new conversations on purpose: its successor owns
        // them now. The address this view holds is simply stale, so it is dropped and asked for again
        // rather than shown to the person as a failure (measured 2026-08-26: a window that had been open
        // across an update could not start anything, and said so in our own words about generations).
        if (opened.with.message.includes(CoreTerminal.DRAINING) && attempt === 0) {
          transport.close();
          this.locator.invalidate();
          await this.connect(cols, rows, attempt + 1);
          return;
        }
        throw new Error(opened.with.message);
      }
      if (opened.say !== "terminalOpened") throw new Error(`the Core answered ${opened.say} to a terminal open`);
    } catch (error) {
      transport.close();
      throw error;
    }
    if (this.closed) {
      transport.close();
      return;
    }
    this.transport = transport;
    const pending = this.pending;
    this.pending = [];
    for (const data of pending) this.handleInput(data);
    await this.pump(transport);
  }

  /// Read the view until the service ends or the connection does.
  private async pump(transport: FrameTransport): Promise<void> {
    try {
      for (;;) {
        const response = readResponse(JSON.parse((await transport.receive()).toString("utf8")));
        switch (response.say) {
          case "terminalOutput":
            this.writeEmitter.fire(this.decoder.decode(Buffer.from(response.with.bytes, "base64"), { stream: true }));
            break;
          case "terminalLagged":
            // The Core re-sends the whole screen next; clear so the redraw lands on a clean page, and start
            // decoding afresh so a multibyte tail cut off by the lag never bleeds into it.
            this.decoder = new TextDecoder("utf-8");
            this.writeEmitter.fire("\x1b[2J\x1b[H");
            break;
          case "terminalExited":
            // A clean exit closes the tab like a shell's would. Anything else keeps the tab, with the
            // service's own last words on it: a resume the service refused (measured: an empty stored
            // conversation exits at once) must not vanish before the person can read why.
            if (response.with.code === 0) {
              this.end(0);
            } else {
              this.writeEmitter.fire(`
\x1b[2m[${this.target.provider} ended with code ${response.with.code}]\x1b[0m
`);
              this.detach();
            }
            return;
          case "failed":
            this.writeEmitter.fire(`\r\n\x1b[31m${response.with.message}\x1b[0m\r\n`);
            this.detach();
            return;
          default:
            this.writeEmitter.fire(`\r\n\x1b[31mthe Core sent ${response.say} on a terminal view\x1b[0m\r\n`);
            this.detach();
            return;
        }
      }
    } catch (error) {
      if (this.closed) return;
      const message = error instanceof Error ? error.message : String(error);
      this.writeEmitter.fire(`\r\n\x1b[31mthe terminal view ended: ${message}\x1b[0m\r\n`);
      this.detach();
    }
  }

  private async send(request: Request): Promise<void> {
    const transport = this.transport;
    if (!transport) return;
    try {
      await transport.send(request);
    } catch (error) {
      if (this.closed) return;
      const message = error instanceof Error ? error.message : String(error);
      this.writeEmitter.fire(`\r\n\x1b[31mthe terminal view ended: ${message}\x1b[0m\r\n`);
      this.detach();
    }
  }

  private end(code?: number): void {
    if (this.closed) return;
    this.detach();
    this.closeEmitter.fire(code);
  }

  /// Stop carrying the view but leave the tab open, so what the service wrote last stays readable.
  private detach(): void {
    this.closed = true;
    this.transport?.close();
    this.transport = null;
  }
}
