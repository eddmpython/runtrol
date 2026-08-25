import * as vscode from "vscode";

import { conversations, type Conversation } from "./conversationList";
import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderCapabilities,
  ProviderLine,
  ProviderUsageGauge,
  SessionLine,
  WatchCursor,
} from "./runtimeTypes";
import { NO_ACTIVITY, type SessionActivity } from "./sessionActivity";
import { discoveryNotice, incompleteDiscovery, providerRowsEqual, sessionRowsEqual } from "./stateRows";
import type { IsolatedWorkspaceLine } from "./protocol";
import { workspaceIdentity } from "./workspaceCollision";

export type RuntimeStateChange = "rows" | "selection" | "usage";

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

  readonly onDidChange = this.changedEmitter.event;

  /// `projectlessRoot` is the scratch folder conversations without a project run in (null when this
  /// surface has none). Held here because every derived row reads it, and one place answering "is this
  /// conversation projectless" keeps the sidebar, the switcher and the tabs in agreement.
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
    this.changedEmitter.fire("usage");
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
    return incompleteDiscovery([...this.nativeCatalogues.values()], this.providerRows);
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
    );
    return this.conversationRows;
  }

  /// Remember which conversations are pinned, then repaint so the order changes at once.
  setPinnedKeys(keys: ReadonlySet<string>): void {
    this.pinnedKeys = keys;
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
    this.changedEmitter.fire("rows");
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
