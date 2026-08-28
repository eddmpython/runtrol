import {
  PUBLIC_LIMITS,
  newMutationRequestId,
  type TerminalControlLease,
  type TerminalView,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";
import type { Conversation, StartedConversation } from "./conversationList";
import { tabColorId } from "./projectColor";
import { tabName } from "./tabName";
import { HIDE_CURSOR, hasVisibleText, MARK_FRAME_MS, paintMark, SHOW_CURSOR } from "./openingMark";
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
  private readonly focusing: vscode.Disposable;
  /// The name this tab is currently showing, so a rename runs once rather than on every focus.
  private readonly named = new Map<vscode.Terminal, string>();

  constructor(
    private readonly runtime: StudioRuntimeClient,
    /// The glyph for a conversation's service, drawn on the tab so two services' tabs tell apart at a glance.
    private readonly iconFor: (conversation: Conversation) => vscode.ThemeIcon | vscode.Uri,
    /// The same glyph by service id, for a fresh conversation that has no row yet.
    private readonly iconForProvider: (providerId: string) => vscode.ThemeIcon | vscode.Uri,
    /// Told whenever the set of not-yet-described conversations changes, so the list redraws at once.
    private readonly startedChanged: () => void = () => undefined,
    /// The same conversation after the provider catalogue was read again, or null when it is gone. A row's
    /// resume proof is signed by one Runtime generation; after an update the proof the row still holds is
    /// refused, and the honest answer is to look again rather than to tell the person to reload.
    private readonly refreshed: (conversationKey: string) => Promise<Conversation | null> = async () => null,
    /// Told every time the service writes to a conversation's screen, which is how the sidebar knows the row
    /// is working: the Runtime's lifecycle only reports turns Runtrol itself started, and it starts none.
    private readonly serviceWrote: (conversationKey: string) => void = () => undefined,
    /// The name this conversation carries now, or null when nothing in the list matches this tab. A tab opened
    /// before the service named the conversation is called after its folder, and the name arrives later.
    private readonly nameOf: (conversationKey: string) => string | null = () => null,
  ) {
    this.closing = vscode.window.onDidCloseTerminal((terminal) => {
      for (const [key, open] of this.open) {
        if (open === terminal) this.open.delete(key);
      }
      // A tab closed before its service named the conversation takes its placeholder row with it. Leaving the row
      // would leave a conversation on screen that nothing on this machine can open.
      if (this.started.delete(terminal)) this.startedChanged();
      this.named.delete(terminal);
    });
    // A tab cannot be renamed through the API, and the editor's own rename command acts on the active terminal.
    // So the name is corrected the moment a person looks at the tab, which is the only moment it matters and the
    // only one that steals no focus. A conversation opened before its service named it is called after its
    // folder until then (operator, 2026-08-28: the service gave it a name and the tab kept the old one).
    this.focusing = vscode.window.onDidChangeActiveTerminal((terminal) => {
      if (terminal) void this.correctName(terminal);
    });
  }

  /// Rename the active tab to the conversation's current name, once.
  private async correctName(terminal: vscode.Terminal): Promise<void> {
    const key = this.keyOf(terminal);
    if (key === null) return;
    const current = this.nameOf(key);
    if (!current) return;
    const wanted = tabName(current);
    if (this.named.get(terminal) === wanted || terminal.name === wanted) {
      this.named.set(terminal, wanted);
      return;
    }
    this.named.set(terminal, wanted);
    await vscode.commands.executeCommand("workbench.action.terminal.renameWithArg", { name: wanted });
  }

  /// Which conversation a tab is showing, by the key the sidebar uses for it.
  private keyOf(terminal: vscode.Terminal): string | null {
    for (const [key, open] of this.open) {
      if (open === terminal) return key;
    }
    const pending = this.started.get(terminal);
    return pending ? `started:${encodeURIComponent(pending.id)}` : null;
  }

  /// Show the conversation's terminal: the tab that already shows it, or a new one beside the active editor.
  show(conversation: Conversation, preserveFocus: boolean): vscode.Terminal {
    const existing = this.open.get(conversation.key);
    if (existing) {
      existing.show(preserveFocus);
      return existing;
    }
    const key = conversation.key;
    const pty = new RuntimeTerminal(this.runtime, targetOf(conversation), () => this.serviceWrote(key), async () => {
      const again = await this.refreshed(conversation.key);
      return again ? targetOf(again) : null;
    });
    // The tab is named for the conversation and coloured for its project. The name answers "which conversation",
    // the colour answers "whose project", and the two together fit in the width a tab actually has.
    const colour = conversation.projectless ? null : tabColorId(conversation.workspace);
    const terminal = vscode.window.createTerminal({
      name: tabName(conversation.title),
      iconPath: tabIcon(colour, () => this.iconFor(conversation)),
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
    // A fresh conversation carries no resume proof, so there is nothing to refresh if the open is refused.
    // The same identity the row will carry, so the signal lands on the row a person is looking at.
    const startedId = `${providerId}:${this.nextStarted + 1}`;
    const startedKey = `started:${encodeURIComponent(startedId)}`;
    const pty = new RuntimeTerminal(this.runtime, {
      provider: providerId,
      native: null,
      workspace,
      blocked: null,
    }, () => this.serviceWrote(startedKey), async () => null);
    const colour = projectless ? null : tabColorId(workspace);
    const terminal = vscode.window.createTerminal({
      name: tabName(name),
      iconPath: tabIcon(colour, () => this.iconForProvider(providerId)),
      color: colour ? new vscode.ThemeColor(colour) : undefined,
      pty,
      location: vscode.TerminalLocation.Editor,
      isTransient: true,
    });
    this.nextStarted += 1;
    this.started.set(terminal, {
      id: startedId,
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
    this.focusing.dispose();
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

function targetOf(conversation: Conversation): Target {
  const native = conversation.native?.adoptionToken
    ? {
        nativeSessionId: conversation.native.nativeSessionId,
        adoptionToken: conversation.native.adoptionToken,
      }
    : null;
  return {
    provider: conversation.providerId,
    native,
    workspace: conversation.workspace,
    blocked: conversation.session?.nativeSessionId && !native
      ? "This conversation has no current provider resume proof for the public Runtime terminal."
      : null,
  };
}

/// The Runtime's own words when a row's resume proof no longer verifies (`runtime_terminal.rs`).
const STALE_PROOF = "native catalogue observation expired";

/// A VS Code pseudoterminal whose other end is the Core's hosted terminal.
///
/// Output is decoded as streaming UTF-8 (a multi-byte character may straddle two chunks). Input is sent as
/// the UTF-8 bytes of what VS Code hands over, which for mouse reports and special keys is already the
/// terminal's own escape vocabulary.
/// The tab's glyph: the project's colour when it has one, the service's own mark when it does not.
///
/// VS Code tints a tab icon only when the icon is one of its own; a file handed to `iconPath` is drawn as the
/// image it is and the `color` beside it does nothing (measured 2026-08-28: the tab kept the service's brand
/// colour while the sidebar row beside it carried the project's). The operator asked twice for the project's
/// colour to reach the tab, so a project's conversation trades the brand mark for the colour that says whose
/// work the tab belongs to. A conversation with no project keeps the mark, having no colour to show instead.
function tabIcon(colour: string | null, service: () => vscode.ThemeIcon | vscode.Uri): vscode.ThemeIcon | vscode.Uri {
  return colour === null ? service() : new vscode.ThemeIcon("comment-discussion");
}

/// Whether a refusal is the control lease being gone rather than the action being wrong.
///
/// Named by the Runtime, so the word is matched rather than guessed at. A lease can end by expiring or by
/// another window taking control, and both are answered the same way: ask for control again.
function leaseLost(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("leaseExpired")
    || message.includes("the terminal control lease expired or was released");
}

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
  /// The mark turning in the middle of the pane until the service's own screen arrives.
  private opening: ReturnType<typeof setInterval> | null = null;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    private target: Target,
    /// Told whenever the service writes, so the sidebar can turn this conversation's icon while it works.
    private readonly wrote: () => void,
    private readonly refreshTarget: () => Promise<Target | null>,
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
    this.startOpeningMark();
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

  /// Turn the mark until there is something real to draw.
  private startOpeningMark(): void {
    if (this.opening) return;
    let at = 0;
    this.writeEmitter.fire(HIDE_CURSOR + paintMark(at, this.dimensions.columns, this.dimensions.rows));
    this.opening = setInterval(() => {
      at += 1;
      this.writeEmitter.fire(paintMark(at, this.dimensions.columns, this.dimensions.rows));
    }, MARK_FRAME_MS);
  }

  /// Pass on what the service drew, and take the mark down at the first sight of it.
  ///
  /// The Runtime answering is not the end of the wait: it hands back a screen that is empty until the CLI
  /// itself writes, and taking the mark down then left the same blank rectangle it exists to prevent
  /// (measured 2026-08-28, one click in a real window).
  private writeFromService(text: string): void {
    if (text.length > 0) this.wrote();
    // Only something a person can see takes the mark down. The Runtime answers with the terminal's screen as
    // it stands, and for a conversation the service has not drawn yet that screen is escape sequences and
    // blanks: taking the mark down on those put the empty rectangle back that the mark exists to prevent
    // (measured 2026-08-28, one click in a real window).
    if (hasVisibleText(text)) this.stopOpeningMark();
    this.writeEmitter.fire(text);
  }

  /// Stop and leave the pane clear. The cursor comes back because the CLI about to draw here expects it.
  private stopOpeningMark(): void {
    if (!this.opening) return;
    clearInterval(this.opening);
    this.opening = null;
    this.writeEmitter.fire(`[2J[H${SHOW_CURSOR}`);
  }

  private async connect(): Promise<void> {
    if (this.target.blocked) throw new Error(this.target.blocked);
    const geometry = this.dimensions;
    const view = await this.openOnce(geometry).catch(async (error: unknown) => {
      // A resume proof signed by an earlier Runtime generation. Read the catalogue again and try once with the
      // proof it hands back now; anything else, or a second refusal, is reported as it came.
      if (!(error instanceof Error) || !error.message.includes(STALE_PROOF)) throw error;
      const fresh = await this.refreshTarget();
      if (!fresh?.native) throw error;
      this.target = fresh;
      return this.openOnce(geometry);
    });
    if (this.closed) {
      view.close();
      return;
    }
    this.view = view;
    this.lease = view.opened.controlLease ?? null;
    this.writeFromService(this.decoder.decode(view.initialScreen, { stream: true }));
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

  /// Read the view until the service ends. A transport break reattaches only to the exact recorded generation and
  /// starts again from its replacement screen snapshot. It never repeats terminal input or redirects the identity.
  private openOnce(geometry: { columns: number; rows: number }): Promise<TerminalView> {
    return this.runtime.openTerminal({
      requestId: newMutationRequestId(),
      providerId: this.target.provider,
      workspace: this.target.workspace,
      target: this.target.native
        ? { kind: "native", ...this.target.native }
        : { kind: "fresh" },
      geometry,
    });
  }

  private async pump(initialView: TerminalView): Promise<void> {
    let view = initialView;
    for (;;) {
      try {
        const notification = await view.next();
        switch (notification.kind) {
          case "output":
            this.writeFromService(this.decoder.decode(notification.bytes, { stream: true }));
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
      } catch {
        if (this.closed) return;
        view.close();
        view = await this.runtime.attachTerminal(
          view.opened.terminal.runtimeGeneration,
          view.opened.terminal.terminalId,
        );
        if (this.closed) {
          view.close();
          return;
        }
        this.view = view;
        this.lease = null;
        this.decoder = new TextDecoder("utf-8");
        this.writeEmitter.fire("\x1b[2J\x1b[H");
        this.writeEmitter.fire(this.decoder.decode(view.initialScreen, { stream: true }));
      }
    }
  }

  private queueControl(
    action: (view: TerminalView, lease: TerminalControlLease) => Promise<void>,
  ): void {
    const command = this.commandTail.then(async () => {
      if (this.closed) return;
      const view = this.view;
      if (!view) throw new Error("The public Runtime terminal is not connected.");
      try {
        await action(view, await this.ensureControl(view));
      } catch (error: unknown) {
        // The lease lives thirty seconds and is renewed when something is sent, so a conversation nobody typed
        // into for longer answers the next keystroke with `leaseExpired`. That is recoverable and used to reach
        // the person as a red line in their conversation instead (operator, 2026-08-28, with a picture). Another
        // window may also have taken control, and asking again is how this window takes it back.
        if (!leaseLost(error)) throw error;
        this.lease = null;
        await action(view, await this.ensureControl(view));
      }
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
    this.stopOpeningMark();
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
    // A tab closed while it was still opening leaves a timer drawing into an emitter nobody reads.
    if (this.opening) {
      clearInterval(this.opening);
      this.opening = null;
    }
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
