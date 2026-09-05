import type { TerminalDescriptor } from "@runtrol/runtime-client";
import * as vscode from "vscode";
import type { Conversation, StartedConversation } from "./conversationList";
import { projectAccentColor } from "./projectColor";
import { tabName } from "./tabName";

import type { StudioRuntimeClient } from "./runtimeClient";
import {
  type JourneyInputTiming,
  type OutputRecord,
  RuntimeTerminal,
  type TerminalPresentation,
  targetOf,
  terminalIdentity,
} from "./runtimeTerminal";

export type { JourneyInputTiming } from "./runtimeTerminal";

const MAX_RECENT_JOURNEY_ENDS = 32;
const MAX_JOURNEY_END_REASON_CHARS = 512;

export class TerminalTabs implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  readonly onDidChange = this.changedEmitter.event;
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
    private readonly iconFor: (conversation: Conversation, accent: string) => vscode.Uri,
    /// The same glyph by service id, for a fresh conversation that has no row yet.
    private readonly iconForProvider: (providerId: string, accent: string) => vscode.Uri,
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
      const known = this.hosts.has(terminal);
      this.forgetJourneyTerminal(terminal);
      for (const [key, open] of this.open) {
        if (open === terminal) this.open.delete(key);
      }
      // A tab closed before its service named the conversation takes its placeholder row with it. Leaving the row
      // would leave a conversation on screen that nothing on this machine can open.
      if (this.started.delete(terminal)) this.startedChanged();
      this.named.delete(terminal);
      this.hosts.delete(terminal);
      if (known) this.changedEmitter.fire();
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
      this.open.delete(`started:${encodeURIComponent(pending.id)}`);
      this.open.set(key, terminal);
      moved.push(terminal);
    }
    if (moved.length === 0) return;
    this.startedChanged();
    // The tab now has a name to wear. Only the active tab can be renamed, and it is the active one whenever
    // the person is sitting in the conversation they just started, which is the whole of this moment.
    for (const terminal of moved) this.correctName(terminal);
    this.changedEmitter.fire();
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

  /// Where a tab's loading, failure, and exit live once the pane holds provider bytes only: the workbench
  /// progress indicator while the terminal opens, the tab title afterwards, and a notification for a failure the
  /// person has to read (`terminalTransportIntegrity`, Studio presentation).
  private presentationFor(terminalOf: () => vscode.Terminal | null): TerminalPresentation {
    return {
      opening: (work) => {
        void vscode.window.withProgress(
          { location: vscode.ProgressLocation.Window, title: `Opening ${terminalOf()?.name ?? "conversation"}` },
          () => work.catch(() => undefined),
        );
      },
      ended: (code) => {
        const terminal = terminalOf();
        const host = terminal ? this.hosts.get(terminal) : undefined;
        if (terminal && host) host.setName(`${terminal.name} · ended ${code}`);
      },
      failed: (message) => {
        const terminal = terminalOf();
        const host = terminal ? this.hosts.get(terminal) : undefined;
        if (terminal && host) host.setName(`${terminal.name} · failed`);
        void vscode.window.showErrorMessage(message);
      },
    };
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
    this.changedEmitter.fire();
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
    }, this.presentationFor(() => terminal));
    // The tab is named for the conversation and coloured for its project. The name answers "which conversation",
    // the colour answers "whose project", and the two together fit in the width a tab actually has.
    const accent = projectAccentColor(conversation.projectless ? null : conversation.homeWorkspace);
    terminal = vscode.window.createTerminal({
      name: tabName(conversation.title),
      iconPath: this.iconFor(conversation, accent),
      pty,
      location: vscode.TerminalLocation.Editor,
      isTransient: true,
    });
    this.open.set(conversation.key, terminal);
    this.hosts.set(terminal, pty);
    this.changedEmitter.fire();
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
    }, this.presentationFor(() => terminal));
    const accent = projectAccentColor(projectless ? null : workspace);
    terminal = vscode.window.createTerminal({
      name: tabName(name),
      iconPath: this.iconForProvider(providerId, accent),
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
    // Filed under the placeholder's own key from the first moment, so a click on the placeholder row shows this
    // tab rather than starting a second provider (measured 2026-09-05: the row's click opened a second Claude).
    this.open.set(startedKey, terminal);
    this.startedChanged();
    this.changedEmitter.fire();
    terminal.show(false);
    return terminal;
  }

  async setDialogue(conversation: Conversation | null, enabled: boolean): Promise<void> {
    const target = conversation ? this.open.get(conversation.key) : vscode.window.activeTerminal;
    const targetKey = target ? this.journeyTargetByTab.get(target) : undefined;
    const descriptor = conversation?.hostedTerminal
      ?? (targetKey ? this.journeyTargets.get(targetKey) : undefined);
    if (!descriptor || descriptor.origin === "observedMirror" || descriptor.processState !== "running") {
      throw new Error("Open a live Runtrol-managed conversation to change dialogue.");
    }
    const tab = conversation ? this.show(conversation, false) : target;
    tab?.show(false);
    await this.runtime.setTerminalDialogue(descriptor, enabled);
  }

  /// Whether this conversation already has its tab open here.
  isOpen(conversationKey: string): boolean {
    return this.open.has(conversationKey);
  }

  /// Close this conversation's tab here, the way the tab's own close does. The process is not touched.
  closeTab(conversationKey: string): boolean {
    const terminal = this.open.get(conversationKey);
    if (!terminal) return false;
    terminal.dispose();
    return true;
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

  recordJourneyOutput(
    runtimeGeneration: string,
    terminalId: string,
    startText: string,
    endText: string,
    deadlineMs: number,
  ): Promise<OutputRecord> {
    const found = this.journeyHost({ runtimeGeneration, terminalId });
    if (!found) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    return found[1].recordOutput(startText, endText, deadlineMs);
  }

  setJourneyDimensions(runtimeGeneration: string, terminalId: string, columns: number, rows: number): void {
    const found = this.journeyHost({ runtimeGeneration, terminalId });
    if (!found) throw this.journeyUnavailable(runtimeGeneration, terminalId);
    found[1].setDimensions({ columns, rows });
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
    let moved = false;
    for (const row of rows) {
      let terminal = this.open.get(row.key);
      if (!terminal && row.hostedKey && row.hostedKey !== row.key) {
        terminal = this.open.get(row.hostedKey);
      }
      if (!terminal) continue;
      if (!this.open.has(row.key)) {
        if (row.hostedKey) this.open.delete(row.hostedKey);
        this.open.set(row.key, terminal);
        moved = true;
      }
      this.correctName(terminal);
    }
    // A tab now filed under another key changes what the sidebar says is open.
    if (moved) this.changedEmitter.fire();
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
    this.changedEmitter.dispose();
  }
}
