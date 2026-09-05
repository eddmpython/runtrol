import * as vscode from "vscode";

import { conversations, type Conversation, type StartedConversation } from "./conversationList";
import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderCapabilities,
  ProviderLine,
  ProviderUsageGauge,
  SessionLine,
  TerminalDescriptor,
  WatchCursor,
} from "./runtimeTypes";
import { NO_ACTIVITY, type SessionActivity } from "./sessionActivity";
import { discoveryNotice, incompleteDiscovery, providerRowsEqual, sessionRowsEqual } from "./stateRows";
import type { IsolatedWorkspaceLine } from "./protocol";
import { workspaceIdentity } from "./workspaceCollision";

export type RuntimeStateChange = "rows" | "selection" | "usage";

/// Whether this window has heard from the Core yet.
///
/// An empty list has two completely different reasons and they need two different sentences: nobody has
/// answered us, or the Core answered and this machine really has no coding service. Measured on the
/// operator's window 2026-08-26: a dropped connection drew "No coding-agent CLI was found on this machine"
/// while three coding services were installed and answering the Core. That is a lie about their machine
/// rather than a report about ours.
export type CoreReach = "connecting" | "reached" | "unreachable";

export class RuntimeState implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<RuntimeStateChange>();
  private readonly cursors = new Map<string, WatchCursor>();
  private sessionRows: readonly SessionLine[] = [];
  private providerRows: readonly ProviderLine[] = [];
  /// Every account's latest position against its limits, as the Core last pushed it.
  private usageRows: readonly ProviderUsageGauge[] = [];
  private readonly nativeCatalogues = new Map<string, NativeChatCatalogue>();
  private readonly capabilityRows = new Map<string, ProviderCapabilities>();
  /// What each running conversation is doing, from the activity watch; absent means nothing known.
  private readonly activities = new Map<string, SessionActivity>();
  private isolatedWorkspaceHomes: ReadonlyMap<string, string> = new Map();
  private selectedId: string | null = null;
  private conversationRows: readonly Conversation[] | null = null;
  /// Conversation keys the operator pinned, in whatever order the machine remembered them. A placement choice
  /// the sidebar reads; it never reaches the daemon or the provider.
  private pinnedKeys: ReadonlySet<string> = new Set();
  private renamedTitles: ReadonlyMap<string, string> = new Map();
  private reach: CoreReach = "connecting";

  readonly onDidChange = this.changedEmitter.event;

  /// Whether the Core has answered this window, so an empty tree can say the true reason.
  get coreReach(): CoreReach {
    return this.reach;
  }

  setCoreReach(reach: CoreReach): void {
    if (this.reach === reach) return;
    this.reach = reach;
    this.changedEmitter.fire("rows");
  }

  /// `projectlessRoot` is the scratch folder conversations without a project run in (null when this
  /// surface has none). Held here because every derived row reads it, and one place answering "is this
  /// conversation projectless" keeps the sidebar, the switcher and the tabs in agreement.
  private started: readonly StartedConversation[] = [];
  private memoryBySession: ReadonlyMap<string, number> = new Map();
  private memoryByNative: ReadonlyMap<string, number> = new Map();
  /// Conversations the service is writing to right now that Runtrol does not host, by native identity.
  private activeNative: ReadonlySet<string> = new Set();
  /// Conversations owned by a provider process that Runtrol observed but did not create or attach.
  private observedNative: ReadonlySet<string> = new Set();
  /// Live provider owners with an exact route into their terminal surface.
  private attachableNative: ReadonlySet<string> = new Set();
  private focusableNative: ReadonlySet<string> = new Set();
  /// Prior live owners whose current process proof could not be refreshed. They are blocked but not called live.
  private unconfirmedNative: ReadonlySet<string> = new Set();
  /// Daemon-owned terminals delivered by the structural terminal-index watch.
  private terminalRows: readonly TerminalDescriptor[] = [];
  /// What the terminal watch could not read, generation by generation. Part of why the list is not everything.
  private terminalWarnings: readonly string[] = [];
  private remember: ((catalogues: readonly NativeChatCatalogue[]) => void) | null = null;
  private rememberUsage: ((usage: readonly ProviderUsageGauge[]) => void) | null = null;

  constructor(readonly projectlessRoot: string | null = null) {}

  get sessions(): readonly SessionLine[] {
    return this.sessionRows;
  }

  get providers(): readonly ProviderLine[] {
    return this.providerRows;
  }

  /// The pushed usage snapshot: what the usage strip draws, never asked for on a clock.
  get usage(): readonly ProviderUsageGauge[] {
    return this.usageRows;
  }

  replaceUsage(usage: readonly ProviderUsageGauge[]): void {
    this.usageRows = usage;
    this.rememberUsage?.(usage);
    this.changedEmitter.fire("usage");
  }

  /// Draw the strip the last window left, before anything has been asked.
  ///
  /// Only ever at the start, and only into an empty strip: once the Core has answered this window, its
  /// answer is the truth and a remembered reading must not overwrite it.
  restoreRememberedUsage(usage: readonly ProviderUsageGauge[]): void {
    if (this.usageRows.length > 0) return;
    this.usageRows = usage;
    this.changedEmitter.fire("usage");
  }

  /// Told what to keep for the next window. Set once, at activation.
  onRememberUsage(remember: (usage: readonly ProviderUsageGauge[]) => void): void {
    this.rememberUsage = remember;
  }

  get nativeChats(): readonly NativeChatLine[] {
    return [...this.nativeCatalogues.values()].flatMap((catalogue) => catalogue.chats);
  }

  nativeCatalogue(providerId: string): NativeChatCatalogue | null {
    return this.nativeCatalogues.get(providerId) ?? null;
  }

  providerCapabilities(providerId: string): ProviderCapabilities | null {
    return this.capabilityRows.get(providerId) ?? null;
  }

  /// Why this list is not everything, in each service's own words, or null when it is everything.
  ///
  /// The sentence itself is built by a pure function so it can be tested without an Extension Host.
  get incompleteDiscovery(): string | null {
    return incompleteDiscovery([...this.nativeCatalogues.values()], this.providerRows, this.terminalWarnings);
  }

  /// The terminal listing's own warnings: the Runtime generations this window could not follow.
  get listingWarnings(): readonly string[] {
    return this.terminalWarnings;
  }

  /// The click-free coverage summary printed directly above the list.
  get discoveryNotice(): string | null {
    return discoveryNotice([...this.nativeCatalogues.values()], this.providerRows);
  }

  get selected(): SessionLine | null {
    return this.sessionRows.find((session) => session.sessionId === this.selectedId) ?? null;
  }

  /// Every conversation on this machine, in the order every surface shows them.
  ///
  /// Derived once here rather than in each surface, so the sidebar, the switcher and the open editor tab can never
  /// disagree about what a conversation is called or where it sits.
  get conversations(): readonly Conversation[] {
    this.conversationRows ??= conversations(
      this.sessionRows,
      this.providerRows,
      this.nativeChats,
      this.selectedId,
      this.projectlessRoot,
      this.activities,
      this.isolatedWorkspaceHomes,
      this.pinnedKeys,
      this.renamedTitles,
      this.started,
      // Terminal output is deliberately not an activity source. A live TUI repaints prompts, menus and cursors
      // while its model is idle; only the provider's structural turn state may animate a row.
      new Set(),
      this.activeNative,
      this.observedNative,
      this.terminalRows,
      this.unconfirmedNative,
      this.attachableNative,
      this.focusableNative,
    );
    return this.conversationRows;
  }

  /// What the provider process behind `row` holds in memory right now, or null when nothing measured it.
  ///
  /// A structured session is keyed by its session id; a hosted terminal by provider and native conversation.
  /// Both figures come from the memory poll, which asks the Runtime on a short clock because a figure moves
  /// without any structural change the watches would carry.
  memoryFor(row: Conversation): number | null {
    // A figure is a process's, and the poll that refreshes it stops when the Core is away: the last figure of a
    // conversation whose process ended stayed on its stored row while the Core was down (measured 2026-09-05).
    if (!row.live) return null;
    const session = row.session ? this.memoryBySession.get(row.session.sessionId) : undefined;
    if (session !== undefined) return session;
    const native = row.native?.nativeSessionId;
    if (!native) return null;
    return this.memoryByNative.get(`${row.providerId}:${native}`) ?? null;
  }

  /// The memory poll's latest figures. Repaints only when a figure actually changed.
  setMemory(bySession: ReadonlyMap<string, number>, byNative: ReadonlyMap<string, number>): void {
    if (sameEntries(bySession, this.memoryBySession) && sameEntries(byNative, this.memoryByNative)) return;
    this.memoryBySession = bySession;
    this.memoryByNative = byNative;
    this.changedEmitter.fire("rows");
  }

  /// The activity poll's latest answer: which conversations have a structurally open model turn.
  ///
  /// This is the sole activity source for provider-owned TUI conversations, hosted or external. It is a
  /// provider structural turn boundary, never an inference from terminal bytes.
  setNativeActivity(active: ReadonlySet<string>): void {
    if (sameMembers(active, this.activeNative)) return;
    this.activeNative = active;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace the cheap provider process roster. A separate set names which exact owners are attachable, so an
  /// unrouteable live row must never start a duplicate resume process.
  setObservedNative(live: ReadonlySet<string>): void {
    if (sameMembers(live, this.observedNative)) return;
    this.observedNative = live;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace the live owners whose provider roster publishes a safe terminal route.
  setAttachableNative(identities: ReadonlySet<string>): void {
    if (sameMembers(identities, this.attachableNative)) return;
    this.attachableNative = identities;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace the live owners whose terminal a registered VS Code window is proved to own.
  setFocusableNative(identities: ReadonlySet<string>): void {
    if (sameMembers(identities, this.focusableNative)) return;
    this.focusableNative = identities;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace owner identities whose latest roster read failed. This is a deny state, not a running state.
  setUnconfirmedNative(identities: ReadonlySet<string>): void {
    if (sameMembers(identities, this.unconfirmedNative)) return;
    this.unconfirmedNative = identities;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace the hosted terminal registry. Process birth and exit arrive here by push, never by the memory poll.
  setTerminals(terminals: readonly TerminalDescriptor[]): void {
    if (
      terminals.length === this.terminalRows.length
      && terminals.every((terminal, index) => terminalRowEqual(terminal, this.terminalRows[index]))
    ) return;
    this.terminalRows = terminals;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Replace what the terminal watch could not read. A Runtime generation it cannot follow holds
  /// conversations this list cannot show, and the reader is told so where the list explains itself.
  setTerminalWarnings(warnings: readonly string[]): void {
    if (
      warnings.length === this.terminalWarnings.length
      && warnings.every((warning, index) => warning === this.terminalWarnings[index])
    ) return;
    this.terminalWarnings = warnings;
    this.changedEmitter.fire("rows");
  }

  /// Remember which conversations are pinned, then repaint so the order changes at once.
  setPinnedKeys(keys: ReadonlySet<string>): void {
    this.pinnedKeys = keys;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// The conversations this window opened that no service has described yet.
  ///
  /// Held here rather than in the tree because the switcher and the tabs read the same list: one place decides
  /// what a conversation is, and a conversation the person just started is one.
  setStarted(started: readonly StartedConversation[]): void {
    this.started = started;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Remember the local nicknames, then repaint so the new names show at once.
  setRenamedTitles(titles: ReadonlyMap<string, string>): void {
    this.renamedTitles = titles;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// What a conversation is doing right now, as the activity watch last reduced it.
  activity(sessionId: string): SessionActivity {
    return this.activities.get(sessionId) ?? NO_ACTIVITY;
  }

  /// The activity watch's coalesced update: several sessions at once, one repaint.
  setActivities(updates: ReadonlyArray<readonly [string, SessionActivity]>): void {
    let changed = false;
    for (const [sessionId, activity] of updates) {
      const current = this.activities.get(sessionId) ?? NO_ACTIVITY;
      if (current.tool === activity.tool && current.signInNeeded === activity.signInNeeded) continue;
      if (activity.tool === null && !activity.signInNeeded) {
        this.activities.delete(sessionId);
      } else {
        this.activities.set(sessionId, activity);
      }
      changed = true;
    }
    if (!changed) return;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  conversationOf(sessionId: string): Conversation | null {
    return this.conversations.find((row) => row.session?.sessionId === sessionId) ?? null;
  }

  replace(sessions: readonly SessionLine[], providers: readonly ProviderLine[]): void {
    const providersChanged = !providerRowsEqual(this.providerRows, providers);
    if (sessionRowsEqual(this.sessionRows, sessions) && !providersChanged) {
      return;
    }
    this.sessionRows = sessions;
    this.providerRows = providers;
    this.conversationRows = null;
    const providerIds = new Set(providers.map((provider) => provider.providerId));
    for (const providerId of this.nativeCatalogues.keys()) {
      if (!providerIds.has(providerId)) this.nativeCatalogues.delete(providerId);
    }
    for (const providerId of this.capabilityRows.keys()) {
      if (!providerIds.has(providerId)) this.capabilityRows.delete(providerId);
    }
    if (this.selectedId && !sessions.some((session) => session.sessionId === this.selectedId)) {
      this.selectedId = null;
    }
    const listed = new Set(sessions.map((session) => session.sessionId));
    for (const sessionId of this.activities.keys()) {
      if (!listed.has(sessionId)) this.activities.delete(sessionId);
    }
    this.changedEmitter.fire("rows");
  }

  select(session: string | null): void {
    if (this.selectedId === session) {
      return;
    }
    this.selectedId = session;
    this.conversationRows = null;
    this.changedEmitter.fire("selection");
  }

  setNativeCatalogue(catalogue: NativeChatCatalogue): void {
    this.nativeCatalogues.set(catalogue.providerId, catalogue);
    this.conversationRows = null;
    this.remember?.([...this.nativeCatalogues.values()]);
    this.changedEmitter.fire("rows");
  }

  /// Draw a list before anything has been asked, from what the last window drew.
  ///
  /// Only ever at the start, and only into an empty state: once a service has answered this window, its own
  /// answer is the truth and a remembered list must not overwrite it.
  restoreRemembered(catalogues: readonly NativeChatCatalogue[]): void {
    if (this.nativeCatalogues.size > 0) return;
    for (const catalogue of catalogues) this.nativeCatalogues.set(catalogue.providerId, catalogue);
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  /// Told what to keep for the next window. Set once, at activation.
  onRemember(remember: (catalogues: readonly NativeChatCatalogue[]) => void): void {
    this.remember = remember;
  }

  /// Drop one provider conversation from its catalogue, as the provider's own answer to a deletion says it
  /// is gone. Returns the catalogue as it was, so a deletion that then fails can put the row back, or null
  /// when nothing was listed to drop.
  forgetNativeChat(providerId: string, nativeSessionId: string): NativeChatCatalogue | null {
    const catalogue = this.nativeCatalogues.get(providerId);
    if (!catalogue || !catalogue.chats.some((chat) => chat.nativeSessionId === nativeSessionId)) return null;
    this.nativeCatalogues.set(providerId, {
      ...catalogue,
      chats: catalogue.chats.filter((chat) => chat.nativeSessionId !== nativeSessionId),
    });
    // The rows already derived lose exactly this one instead of being derived again: measured
    // 2026-08-25, deriving every row of a machine with 130 conversations took about 20 ms, which
    // was the whole of the time the deleted row stayed on screen.
    this.conversationRows = this.conversationRows?.filter(
      (row) => !(row.providerId === providerId && row.native?.nativeSessionId === nativeSessionId),
    ) ?? null;
    this.changedEmitter.fire("rows");
    return catalogue;
  }

  setProviderCapabilities(capabilities: ProviderCapabilities): void {
    this.capabilityRows.set(capabilities.providerId, capabilities);
    this.changedEmitter.fire("rows");
  }

  setIsolatedWorkspaces(workspaces: readonly IsolatedWorkspaceLine[]): void {
    const next = new Map(workspaces.map((workspace) => [
      workspaceIdentity(workspace.workspace),
      workspace.project,
    ]));
    if (
      next.size === this.isolatedWorkspaceHomes.size
      && [...next].every(([key, value]) => this.isolatedWorkspaceHomes.get(key) === value)
    ) {
      return;
    }
    this.isolatedWorkspaceHomes = next;
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  clearNativeCatalogues(): void {
    if (this.nativeCatalogues.size === 0 && this.capabilityRows.size === 0) return;
    this.nativeCatalogues.clear();
    this.capabilityRows.clear();
    this.conversationRows = null;
    this.changedEmitter.fire("rows");
  }

  cursor(session: string): WatchCursor | null {
    return this.cursors.get(session) ?? null;
  }

  advance(session: string, cursor: WatchCursor): void {
    this.cursors.set(session, cursor);
  }

  // Called when the conversation document is reborn (a reset cleared its DOM). Watching without a
  // cursor replays the daemon's bounded recent window into the fresh document, where resuming from
  // the last delivered cursor would leave everything already seen invisible. Backpressure restarts
  // keep their cursor: the document survived, so a replay there would duplicate what is on screen.
  forgetCursor(session: string): void {
    this.cursors.delete(session);
  }

  dispose(): void {
    this.cursors.clear();
    this.activities.clear();
    this.nativeCatalogues.clear();
    this.capabilityRows.clear();
    this.changedEmitter.dispose();
  }
}

/// Whether two sets hold the same names, so a poll that answered the same thing repaints nothing.
function sameMembers(left: ReadonlySet<string>, right: ReadonlySet<string>): boolean {
  if (left.size !== right.size) return false;
  for (const name of left) {
    if (!right.has(name)) return false;
  }
  return true;
}

function sameEntries(left: ReadonlyMap<string, number>, right: ReadonlyMap<string, number>): boolean {
  if (left.size !== right.size) return false;
  for (const [key, value] of left) {
    if (right.get(key) !== value) return false;
  }
  return true;
}

function terminalRowEqual(left: TerminalDescriptor, right: TerminalDescriptor | undefined): boolean {
  return right !== undefined
    && left.terminalId === right.terminalId
    && left.terminalGeneration === right.terminalGeneration
    && left.runtimeGeneration === right.runtimeGeneration
    && left.providerId === right.providerId
    && left.nativeSessionId === right.nativeSessionId
    && left.workspace === right.workspace
    && left.spawnedBy === right.spawnedBy
    && left.projectRoot === right.projectRoot
    && left.initialMessageId === right.initialMessageId
    && left.dialogueEnabled === right.dialogueEnabled
    && left.processState === right.processState
    && left.openedAtMs === right.openedAtMs
    && left.geometry.columns === right.geometry.columns
    && left.geometry.rows === right.geometry.rows
    && left.controlGeneration === right.controlGeneration
    && left.controlHeld === right.controlHeld;
}
