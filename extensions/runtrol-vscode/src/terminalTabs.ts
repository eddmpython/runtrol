import {
  PUBLIC_LIMITS,
  newMutationRequestId,
  type TerminalControlLease,
  type TerminalDescriptor,
  type TerminalView,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";
import type { Conversation, StartedConversation } from "./conversationList";
import { tabColorId } from "./projectColor";
import { tabName } from "./tabName";

import { HIDE_CURSOR, hasVisibleText, MARK_FRAME_MS, paintMark, SHOW_CURSOR } from "./openingMark";
import type { StudioRuntimeClient } from "./runtimeClient";

const MAX_PENDING_INPUT_ACTIONS = 256;
const MAX_RECENT_JOURNEY_ENDS = 32;
const MAX_JOURNEY_END_REASON_CHARS = 512;

export type JourneyInputTiming = {
  receivedAtMs: number;
  dispatchedAtMs: number;
  acknowledgedAtMs: number;
};

type InputMeasurement = {
  receivedAtMs: number;
  resolve(timing: JourneyInputTiming): void;
  reject(error: unknown): void;
};

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
  /// The extension PTY behind each tab, which owns the public name-change event for active and inactive tabs.
  private readonly hosts = new Map<vscode.Terminal, RuntimeTerminal>();
  /// Exact public identities returned for open tabs, retained until that tab closes even if its watch drops.
  private readonly journeyTargets = new Map<string, TerminalDescriptor>();
  private readonly journeyTargetByTab = new Map<vscode.Terminal, string>();
  /// Bounded test diagnostics for a terminal that ended before an installed-host assertion reached it.
  /// Reasons contain lifecycle metadata only, never terminal output or a conversation transcript.
  private readonly journeyEnds = new Map<string, string>();
  /// Conversations opened here that no service has described yet, by the tab showing each one.
  private readonly started = new Map<vscode.Terminal, StartedConversation>();
  private nextStarted = 0;
  private readonly closing: vscode.Disposable;
  /// The name this tab is currently showing, so repeated row snapshots emit no duplicate name change.
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
    /// Told every time the service writes to a conversation's screen, for output-adjacent bookkeeping such as
    /// refreshing project changes. This is never model activity: an idle TUI also repaints prompts and cursors.
    private readonly serviceOutput: (conversationKey: string) => void = () => undefined,
    /// The name this conversation carries now, or null when nothing in the list matches this tab. A tab opened
    /// before the service named the conversation is called after its folder, and the name arrives later.
    private readonly nameOf: (conversationKey: string) => string | null = () => null,
  ) {
    this.closing = vscode.window.onDidCloseTerminal((terminal) => {
      this.forgetJourneyTerminal(terminal);
      for (const [key, open] of this.open) {
        if (open === terminal) this.open.delete(key);
      }
      // A tab closed before its service named the conversation takes its placeholder row with it. Leaving the row
      // would leave a conversation on screen that nothing on this machine can open.
      if (this.started.delete(terminal)) this.startedChanged();
      this.named.delete(terminal);
      this.hosts.delete(terminal);
    });
  }

  /// The tab this conversation is already running in, opened before the service named it.
  ///
  /// Move the tabs whose conversation the service has just named onto their real conversation.
  ///
  /// A tab opened from here is filed under a placeholder until then. Leaving it there costs two visible
  /// things: the row's next click opens a second tab on a conversation already on screen, and the tab keeps
  /// the folder name for the rest of its life because the placeholder it is filed under is no longer in the
  /// list to have a name. Which placeholder became which conversation is `namedPlaceholders`, the same answer
  /// the list uses to drop it.
  retire(named: ReadonlyMap<string, string>): void {
    const moved: vscode.Terminal[] = [];
    for (const [terminal, pending] of [...this.started]) {
      const key = named.get(pending.id);
      if (key === undefined) continue;
      this.started.delete(terminal);
      this.open.set(key, terminal);
      moved.push(terminal);
    }
    if (moved.length === 0) return;
    this.startedChanged();
    // The tab now has a name to wear. Only the active tab can be renamed, and it is the active one whenever
    // the person is sitting in the conversation they just started, which is the whole of this moment.
    for (const terminal of moved) this.correctName(terminal);
  }

  /// Rename any tab to the conversation's current name through its public pseudoterminal event.
  private correctName(terminal: vscode.Terminal): void {
    const key = this.keyOf(terminal);
    if (key === null) return;
    const current = this.nameOf(key);
    if (!current) return;
    const wanted = tabName(current);
    if (this.named.get(terminal) === wanted || terminal.name === wanted) {
      this.named.set(terminal, wanted);
      return;
    }
    const host = this.hosts.get(terminal);
    if (!host) return;
    this.named.set(terminal, wanted);
    host.setName(wanted);
  }

  /// Which conversation a tab is showing, by the key the sidebar uses for it.
  private keyOf(terminal: vscode.Terminal): string | null {
    for (const [key, open] of this.open) {
      if (open === terminal) return key;
    }
    const pending = this.started.get(terminal);
    return pending ? `started:${encodeURIComponent(pending.id)}` : null;
  }

  /// Stop routing future clicks to a terminal whose Runtime view has ended, while leaving its final screen in
  /// the editor for diagnosis. A later click can then make a fresh connection instead of revealing a dead tab.
  private retireDisconnected(terminal: vscode.Terminal, reason: string): void {
    const journeyKey = this.journeyTargetByTab.get(terminal);
    if (journeyKey !== undefined) this.rememberJourneyEnd(journeyKey, reason);
    for (const [key, open] of this.open) {
      if (open === terminal) this.open.delete(key);
    }
    if (this.started.delete(terminal)) this.startedChanged();
    this.named.delete(terminal);
    this.hosts.delete(terminal);
  }

  /// Show the conversation's terminal: the tab that already shows it, or a new one beside the active editor.
  show(conversation: Conversation, preserveFocus: boolean): vscode.Terminal {
    const existing = this.open.get(conversation.key)
      ?? (conversation.hostedKey ? this.open.get(conversation.hostedKey) : undefined);
    if (existing) {
      if (!this.open.has(conversation.key)) {
        if (conversation.hostedKey) this.open.delete(conversation.hostedKey);
        this.open.set(conversation.key, existing);
      }
      this.correctName(existing);
      existing.show(preserveFocus);
      return existing;
    }
    const key = conversation.key;
    let terminal: vscode.Terminal | null = null;
    const pty = new RuntimeTerminal(this.runtime, targetOf(conversation), () => this.serviceOutput(key), async () => {
      const again = await this.refreshed(conversation.key);
      return again ? targetOf(again) : null;
    }, (descriptor) => {
      if (terminal) this.rememberJourneyTerminal(terminal, descriptor);
    }, (reason) => {
      if (terminal) this.retireDisconnected(terminal, reason);
    });
    // The tab is named for the conversation and coloured for its project. The name answers "which conversation",
    // the colour answers "whose project", and the two together fit in the width a tab actually has.
    const colour = conversation.projectless ? null : tabColorId(conversation.homeWorkspace);
    terminal = vscode.window.createTerminal({
      name: tabName(conversation.title),
      iconPath: tabIcon(colour, () => this.iconFor(conversation)),
      color: colour ? new vscode.ThemeColor(colour) : undefined,
      pty,
      location: vscode.TerminalLocation.Editor,
      isTransient: true,
    });
    this.open.set(conversation.key, terminal);
    this.hosts.set(terminal, pty);
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
    let terminal: vscode.Terminal | null = null;
    const pty = new RuntimeTerminal(this.runtime, {
      provider: providerId,
      native: null,
      hosted: null,
      workspace,
      blocked: null,
    }, () => this.serviceOutput(startedKey), async () => null, (descriptor) => {
      if (terminal) {
        this.rememberJourneyTerminal(terminal, descriptor);
        this.bindStartedTerminal(terminal, descriptor);
      }
    }, (reason) => {
      if (terminal) this.retireDisconnected(terminal, reason);
    });
    const colour = projectless ? null : tabColorId(workspace);
    terminal = vscode.window.createTerminal({
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
    this.hosts.set(terminal, pty);
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

  /// Bind a fresh placeholder to the public terminal identity Runtime actually opened.
  private bindStartedTerminal(terminal: vscode.Terminal, descriptor: TerminalDescriptor): void {
    const pending = this.started.get(terminal);
    if (
      !pending
      || (pending.runtimeGeneration === descriptor.runtimeGeneration && pending.terminalId === descriptor.terminalId)
    ) return;
    this.started.set(terminal, {
      ...pending,
      runtimeGeneration: descriptor.runtimeGeneration,
      terminalId: descriptor.terminalId,
    });
    this.startedChanged();
  }

  private rememberJourneyTerminal(terminal: vscode.Terminal, descriptor: TerminalDescriptor): void {
    this.forgetJourneyTerminal(terminal);
    const key = terminalIdentity(descriptor.runtimeGeneration, descriptor.terminalId);
    this.journeyTargetByTab.set(terminal, key);
    this.journeyTargets.set(key, descriptor);
    this.journeyEnds.delete(key);
  }

  private rememberJourneyEnd(key: string, reason: string): void {
    this.journeyEnds.delete(key);
    this.journeyEnds.set(key, reason.slice(0, MAX_JOURNEY_END_REASON_CHARS));
    while (this.journeyEnds.size > MAX_RECENT_JOURNEY_ENDS) {
      const oldest = this.journeyEnds.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      this.journeyEnds.delete(oldest);
    }
  }

  private forgetJourneyTerminal(terminal: vscode.Terminal): void {
    const key = this.journeyTargetByTab.get(terminal);
    if (key === undefined) return;
    this.journeyTargetByTab.delete(terminal);
    this.journeyTargets.delete(key);
  }

  /// Test-only observation of the real editor-terminal path. The production extension never calls these
  /// methods: the installed-host journey uses them to assert identity and byte delivery without reading or
  /// retaining a conversation transcript.
  async waitForJourneyTerminal(
    target: { providerId: string; workspace: string } | { runtimeGeneration: string; terminalId: string },
    deadlineMs: number,
  ): Promise<TerminalDescriptor> {
    const deadline = Date.now() + deadlineMs;
    for (;;) {
      const found = this.journeyHost(target)?.[1].descriptor() ?? null;
      if (found) return found;
      if (Date.now() >= deadline) {
        throw new Error(`the VS Code terminal did not connect within ${deadlineMs} ms`);
      }
      await new Promise<void>((resolve) => setTimeout(resolve, 20));
    }
  }

  waitForJourneyOutput(
    runtimeGeneration: string,
    terminalId: string,
    text: string,
    deadlineMs: number,
  ): Promise<number> {
    const found = this.journeyHost({ runtimeGeneration, terminalId });
    if (!found) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    return found[1].waitForOutput(text, deadlineMs);
  }

  writeJourneyInput(runtimeGeneration: string, terminalId: string, text: string): Promise<JourneyInputTiming> {
    const found = this.journeyHost({ runtimeGeneration, terminalId });
    if (!found) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    // Use VS Code's public Terminal API. This enters RuntimeTerminal.handleInput exactly as keyboard input does.
    const measured = found[1].measureNextInput();
    found[0].sendText(text, true);
    return measured;
  }

  /// Test-only measurement after VS Code has delivered input to the Pseudoterminal callback. The simultaneous-host
  /// gate measures the public Terminal API bounce separately, then uses this entry to isolate Runtime, PTY, and
  /// fan-out latency without charging VS Code's extension-to-renderer-to-extension test loop to the product path.
  writeDirectJourneyInput(runtimeGeneration: string, terminalId: string, text: string): Promise<JourneyInputTiming> {
    const found = this.journeyHost({ runtimeGeneration, terminalId });
    if (!found) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    return found[1].handleMeasuredInput(text);
  }

  /// Test-only exact stop through the same Runtime terminal identity the public open response returned.
  async stopJourneyTerminal(runtimeGeneration: string, terminalId: string): Promise<void> {
    const descriptor = this.journeyTargets.get(terminalIdentity(runtimeGeneration, terminalId));
    if (!descriptor) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    await this.runtime.stopTerminal(descriptor);
  }

  private journeyUnavailable(runtimeGeneration: string, terminalId: string): Error {
    const reason = this.journeyEnds.get(terminalIdentity(runtimeGeneration, terminalId));
    return new Error(reason
      ? `terminal ${terminalId} ended in this VS Code window: ${reason}`
      : `terminal ${terminalId} is not open in this VS Code window`);
  }

  private journeyHost(
    target: { providerId: string; workspace: string } | { runtimeGeneration: string; terminalId: string },
  ): [vscode.Terminal, RuntimeTerminal] | null {
    for (const [terminal, host] of this.hosts) {
      const descriptor = host.descriptor();
      if (!descriptor) continue;
      if (
        "terminalId" in target
          ? descriptor.runtimeGeneration === target.runtimeGeneration && descriptor.terminalId === target.terminalId
          : descriptor.providerId === target.providerId && descriptor.workspace === target.workspace
      ) {
        return [terminal, host];
      }
    }
    return null;
  }

  /// Move tabs opened from an identity-pending hosted row onto the provider's stable conversation key.
  ///
  /// The terminal index is published before the provider store has a title. When that title arrives the row changes
  /// identity, but the PTY does not: this rekeys the existing tab instead of opening a second viewer.
  reconcileHosted(rows: readonly Conversation[]): void {
    for (const row of rows) {
      let terminal = this.open.get(row.key);
      if (!terminal && row.hostedKey && row.hostedKey !== row.key) {
        terminal = this.open.get(row.hostedKey);
      }
      if (!terminal) continue;
      if (!this.open.has(row.key)) {
        if (row.hostedKey) this.open.delete(row.hostedKey);
        this.open.set(row.key, terminal);
      }
      this.correctName(terminal);
    }
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
    this.hosts.clear();
    this.journeyTargets.clear();
    this.journeyTargetByTab.clear();
    this.journeyEnds.clear();
  }
}

type Target = {
  provider: string;
  native: { nativeSessionId: string; adoptionToken: string } | null;
  hosted: { runtimeGeneration: string; terminalId: string } | null;
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
    hosted: conversation.hostedTerminal
      ? {
          runtimeGeneration: conversation.hostedTerminal.runtimeGeneration,
          terminalId: conversation.hostedTerminal.terminalId,
        }
      : null,
    workspace: conversation.workspace,
    blocked: !conversation.hostedTerminal && conversation.session?.nativeSessionId && !native
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
/// Named by the Runtime, so the word is matched rather than guessed at. Leases are independent per view, but this
/// view can still hold an expired generation or race its own reconnect. Both are answered by asking again.
function leaseLost(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("leaseExpired")
    || message.includes("the terminal control lease expired or was released")
    // A reconnect or overlapping mutation in this same view may leave an older lease generation behind.
    || message.includes("controlConflict");
}

function sameGeometry(
  left: { columns: number; rows: number },
  right: { columns: number; rows: number },
): boolean {
  return left.columns === right.columns && left.rows === right.rows;
}

function terminalIdentity(runtimeGeneration: string, terminalId: string): string {
  return `${runtimeGeneration}:${terminalId}`;
}

class RuntimeTerminal implements vscode.Pseudoterminal {
  private readonly writeEmitter = new vscode.EventEmitter<string>();
  private readonly closeEmitter = new vscode.EventEmitter<number | void>();
  private readonly nameEmitter = new vscode.EventEmitter<string>();
  readonly onDidWrite = this.writeEmitter.event;
  readonly onDidClose = this.closeEmitter.event;
  readonly onDidChangeName = this.nameEmitter.event;
  private view: TerminalView | null = null;
  private lease: TerminalControlLease | null = null;
  private decoder = new TextDecoder("utf-8");
  private closed = false;
  private commandTail = Promise.resolve();
  private dimensions = { columns: 120, rows: 40 };
  private lastResize = { columns: 120, rows: 40 };
  private resizeScheduled = false;
  /// Input typed before the connection is up is kept and sent once it is: a person who starts typing while
  /// the tab opens must not lose the first keys.
  private pending: Uint8Array[] = [];
  private pendingBytes = 0;
  private queuedInputBytes = 0;
  private queuedInputActions = 0;
  private nextInputMeasurement: Omit<InputMeasurement, "receivedAtMs"> | null = null;
  /// The mark standing in the middle of the pane, lit in passes, until the service's own screen arrives.
  private opening: ReturnType<typeof setInterval> | null = null;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    private target: Target,
    /// Told whenever the service writes, for bookkeeping that does not infer model activity.
    private readonly output: () => void,
    private readonly refreshTarget: () => Promise<Target | null>,
    /// The exact hosted terminal identity returned by Runtime, used to name a fresh placeholder safely.
    private readonly connected: (terminal: TerminalDescriptor) => void,
    /// Withdraw this dead route while leaving its final editor contents visible.
    private readonly disconnected: (reason: string) => void,
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

  /// Change the VS Code tab name without focusing it or replacing this view.
  setName(name: string): void {
    if (!this.closed) this.nameEmitter.fire(name);
  }

  descriptor(): TerminalDescriptor | null {
    return this.view?.opened.terminal ?? null;
  }

  waitForOutput(text: string, deadlineMs: number): Promise<number> {
    if (!text) return Promise.reject(new Error("the terminal output marker is empty"));
    return new Promise<number>((resolve, reject) => {
      let tail = "";
      let timer: NodeJS.Timeout | undefined;
      const finish = (result: number | Error): void => {
        if (timer) clearTimeout(timer);
        subscription.dispose();
        if (result instanceof Error) reject(result);
        else resolve(result);
      };
      const subscription = this.onDidWrite((chunk) => {
        const candidate = tail + chunk;
        if (candidate.includes(text)) {
          finish(Date.now());
          return;
        }
        // Only the boundary needed to match a split marker is retained. This is a test latch, not a transcript.
        tail = candidate.slice(-Math.max(0, text.length - 1));
      });
      timer = setTimeout(
        () => finish(new Error(`terminal output did not contain ${JSON.stringify(text)} within ${deadlineMs} ms`)),
        deadlineMs,
      );
    });
  }

  handleInput(data: string): void {
    const measurement = this.takeInputMeasurement();
    if (this.closed) {
      measurement?.reject(new Error("the terminal closed before measured input arrived"));
      return;
    }
    const bytes = Buffer.from(data, "utf8");
    if (bytes.byteLength === 0) {
      measurement?.reject(new Error("the measured terminal input was empty"));
      return;
    }
    if (!this.view) {
      measurement?.reject(new Error("the measured terminal input arrived before the Runtime view connected"));
      if (this.pendingBytes + bytes.byteLength > PUBLIC_LIMITS.maxTerminalWriteBytes) {
        this.fail(new Error("Input entered while the terminal opened exceeded the Runtime input bound."));
        return;
      }
      this.pending.push(bytes);
      this.pendingBytes += bytes.byteLength;
      return;
    }
    this.queueInput(bytes, measurement);
  }

  measureNextInput(): Promise<JourneyInputTiming> {
    if (this.nextInputMeasurement) {
      return Promise.reject(new Error("a measured terminal input is already armed"));
    }
    return new Promise<JourneyInputTiming>((resolve, reject) => {
      this.nextInputMeasurement = { resolve, reject };
    });
  }

  handleMeasuredInput(data: string): Promise<JourneyInputTiming> {
    const measured = this.measureNextInput();
    this.handleInput(data);
    return measured;
  }

  private takeInputMeasurement(): InputMeasurement | null {
    const pending = this.nextInputMeasurement;
    this.nextInputMeasurement = null;
    return pending ? { ...pending, receivedAtMs: Date.now() } : null;
  }

  setDimensions(dimensions: vscode.TerminalDimensions): void {
    const next = { columns: dimensions.columns, rows: dimensions.rows };
    if (sameGeometry(this.dimensions, next)) return;
    this.dimensions = next;
    this.scheduleResize();
  }

  /// Keep the light passing over the mark until there is something real to draw.
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
    // Only something a person can see takes the mark down. The Runtime answers with the terminal's screen as
    // it stands, and for a conversation the service has not drawn yet that screen is escape sequences and
    // blanks: taking the mark down on those put the empty rectangle back that the mark exists to prevent
    // (measured 2026-08-28, one click in a real window).
    if (hasVisibleText(text)) this.stopOpeningMark();
    // Feed VS Code's terminal buffer before rebuilding sidebar state. The activity callback is bookkeeping;
    // it must never sit in front of bytes a person is waiting to see.
    this.writeEmitter.fire(text);
    if (text.length > 0) this.output();
  }

  /// Stop and leave the pane clear. The cursor comes back because the CLI about to draw here expects it.
  private stopOpeningMark(): void {
    if (!this.opening) return;
    clearInterval(this.opening);
    this.opening = null;
    this.writeEmitter.fire(`\x1b[2J\x1b[H${SHOW_CURSOR}`);
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
    this.connected(view.opened.terminal);
    this.lease = view.opened.controlLease ?? null;
    this.lastResize = geometry;
    this.writeFromService(this.decoder.decode(view.initialScreen, { stream: true }));
    if (
      this.dimensions.columns !== geometry.columns
      || this.dimensions.rows !== geometry.rows
    ) {
      this.scheduleResize();
    }
    const pending = this.pending;
    this.pending = [];
    this.pendingBytes = 0;
    for (const bytes of pending) this.queueInput(bytes);
    await this.pump(view);
  }

  /// Preserve exact keystroke order while bounding both byte ownership and Promise ownership if a Runtime stalls.
  private queueInput(bytes: Uint8Array, measurement: InputMeasurement | null = null): void {
    if (
      this.queuedInputBytes + bytes.byteLength > PUBLIC_LIMITS.maxTerminalWriteBytes
      || this.queuedInputActions >= MAX_PENDING_INPUT_ACTIONS
    ) {
      const error = new Error("Pending terminal input exceeded the bounded control queue.");
      measurement?.reject(error);
      this.fail(error);
      return;
    }
    this.queuedInputBytes += bytes.byteLength;
    this.queuedInputActions += 1;
    let dispatchedAtMs = 0;
    this.queueControl(async (view, lease) => {
      dispatchedAtMs = Date.now();
      await view.write({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        bytesBase64: Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString("base64"),
      });
    }, (error) => {
      this.queuedInputBytes = Math.max(0, this.queuedInputBytes - bytes.byteLength);
      this.queuedInputActions = Math.max(0, this.queuedInputActions - 1);
      if (!measurement) return;
      if (error) {
        measurement.reject(error);
      } else if (dispatchedAtMs === 0) {
        measurement.reject(new Error("the terminal closed before measured input dispatch"));
      } else {
        measurement.resolve({
          receivedAtMs: measurement.receivedAtMs,
          dispatchedAtMs,
          acknowledgedAtMs: Date.now(),
        });
      }
    });
  }

  /// Collapse a resize burst to one in-flight request and the latest pending geometry. There is no delay: the
  /// current size enters the control lane immediately, while obsolete intermediate sizes consume no queue or PTY I/O.
  private scheduleResize(): void {
    if (
      this.closed
      || !this.view
      || this.resizeScheduled
      || sameGeometry(this.dimensions, this.lastResize)
    ) return;
    this.resizeScheduled = true;
    let geometry: { columns: number; rows: number } | null = null;
    this.queueControl(async (view, lease) => {
      geometry ??= this.dimensions;
      await view.resize({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        geometry,
      });
      this.lastResize = geometry;
    }, () => {
      this.resizeScheduled = false;
      this.scheduleResize();
    });
  }

  /// Read the view until the service ends. A transport break reattaches only to the exact recorded generation and
  /// starts again from its replacement screen snapshot. It never repeats terminal input or redirects the identity.
  private openOnce(geometry: { columns: number; rows: number }): Promise<TerminalView> {
    if (this.target.hosted) {
      return this.runtime.attachTerminal(
        this.target.hosted.runtimeGeneration,
        this.target.hosted.terminalId,
      );
    }
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
            this.writeFromService(`\x1b[2J\x1b[H${this.decoder.decode(notification.screen, { stream: true })}`);
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
              this.detach(false, `provider terminal exited with code ${notification.exitCode}`);
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
        this.connected(view.opened.terminal);
        this.lease = null;
        this.decoder = new TextDecoder("utf-8");
        this.writeFromService(`\x1b[2J\x1b[H${this.decoder.decode(view.initialScreen, { stream: true })}`);
      }
    }
  }

  private queueControl(
    action: (view: TerminalView, lease: TerminalControlLease) => Promise<void>,
    settled: (error?: unknown) => void = () => undefined,
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
        // reconnect in this view may also have replaced its generation, and asking again is the exact recovery.
        if (!leaseLost(error)) throw error;
        this.lease = null;
        await action(view, await this.ensureControl(view));
      }
    });
    const finish = (error?: unknown): void => {
      try {
        settled(error);
      } catch (settleError: unknown) {
        this.fail(settleError);
      }
    };
    this.commandTail = command.then(
      () => finish(),
      (error: unknown) => finish(error),
    );
    void command.catch((error: unknown) => this.fail(error));
  }

  private async ensureControl(view: TerminalView): Promise<TerminalControlLease> {
    const lease = this.lease;
    if (lease && lease.expiresAtMs > Date.now() + 5_000) return lease;
    // A lease whose time is already up cannot be renewed: the Runtime retires it before answering, so asking
    // to renew is a round trip that fails by construction. Past that moment the only move is to ask for
    // control again, which is what a person typing after a quiet minute is entitled to.
    const renewable = lease !== null && lease.expiresAtMs > Date.now();
    this.lease = renewable && lease
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
    this.detach(false, `Runtime terminal failed: ${message}`);
  }

  private end(code?: number): void {
    if (this.closed) return;
    this.detach(false, `provider terminal exited with code ${code ?? "unknown"}`);
    this.closeEmitter.fire(code);
  }

  /// Stop carrying the view but leave the tab open, so what the service wrote last stays readable.
  private detach(notifyRuntime: boolean, reason = "terminal view disconnected"): void {
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
    if (!notifyRuntime) this.disconnected(reason);
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
