import * as vscode from "vscode";

import { conversations, type Conversation } from "./conversationList";
import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderLine,
  SessionLine,
  WatchCursor,
} from "./runtimeTypes";
import { NO_ACTIVITY, type SessionActivity } from "./sessionActivity";
import { incompleteDiscovery, providerRowsEqual, sessionRowsEqual } from "./stateRows";

export type RuntimeStateChange = "rows" | "selection";

export class RuntimeState implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<RuntimeStateChange>();
  private readonly cursors = new Map<string, WatchCursor>();
  private sessionRows: readonly SessionLine[] = [];
  private providerRows: readonly ProviderLine[] = [];
  private readonly nativeCatalogues = new Map<string, NativeChatCatalogue>();
  /// What each running conversation is doing, from the activity watch; absent means nothing known.
  private readonly activities = new Map<string, SessionActivity>();
  private selectedId: string | null = null;
  private conversationRows: readonly Conversation[] | null = null;

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

  get nativeChats(): readonly NativeChatLine[] {
    return [...this.nativeCatalogues.values()].flatMap((catalogue) => catalogue.chats);
  }

  nativeCatalogue(providerId: string): NativeChatCatalogue | null {
    return this.nativeCatalogues.get(providerId) ?? null;
  }

  /// Why this list is not everything, in each service's own words, or null when it is everything.
  ///
  /// The sentence itself is built by a pure function so it can be tested without an Extension Host.
  get incompleteDiscovery(): string | null {
    return incompleteDiscovery([...this.nativeCatalogues.values()], this.providerRows);
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
    );
    return this.conversationRows;
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
    if (sessionRowsEqual(this.sessionRows, sessions) && providerRowsEqual(this.providerRows, providers)) {
      return;
    }
    this.sessionRows = sessions;
    this.providerRows = providers;
    this.conversationRows = null;
    const providerIds = new Set(providers.map((provider) => provider.providerId));
    for (const providerId of this.nativeCatalogues.keys()) {
      if (!providerIds.has(providerId)) this.nativeCatalogues.delete(providerId);
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

  clearNativeCatalogues(): void {
    if (this.nativeCatalogues.size === 0) return;
    this.nativeCatalogues.clear();
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
    this.changedEmitter.dispose();
  }
}
