import { mkdir } from "node:fs/promises";
import path from "node:path";

import { PUBLIC_LIMITS, type NativeActivity, type PublicInputBlock } from "@runtrol/runtime-client";
import type { TerminalIndexSnapshot, WindowRevealResult } from "@runtrol/runtime-client";
import * as vscode from "vscode";

import { BoundedDedupe } from "./boundedDedupe";
import type { TerminalTabs } from "./terminalTabs";
import { CoreClient } from "./core/client";
import { readGitBranch } from "./gitBranch";
import { IsolatedWorkspaces } from "./isolatedWorkspace";
import { isProjectless } from "./projectlessWorkspace";
import { planProjectDeletion, projectDeletionQuestion } from "./projectDeletion";
import type { ProjectRecord } from "./projects";
import type {
  IsolatedWorkspaceLine,
  ProviderUpdateLine,
} from "./protocol";
import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderCapabilities,
  ProviderLine,
  ModelCatalog,
  SessionLine,
  TerminalDescriptor,
  WorkspaceAccess,
} from "./runtimeTypes";
import { abortableDelay } from "./abortableDelay";

/// How long a stopped conversation is given to leave this window's live rows before it is kept rather than
/// deleted. The Core sees an exit within a few hundred milliseconds; ten seconds covers a slow machine.
const STOP_SETTLE_MS = 10_000;
import type { Conversation } from "./conversationList";
import { attentionCount, nextNeedingYou, projects, runningElsewhere, nativeProcessKey, namedPlaceholders } from "./conversationList";
import { conversationDeletion, deletionQuestion } from "./conversationDeletion";
import { editorPanelFor } from "./editorPanels";
import { archivalQuestion, conversationArchival } from "./conversationArchival";
import { awaitsVerification, isUsable, unaskedUsable } from "./providerHealth";
import type { HelpOffer, ServiceTrouble } from "./serviceHelp";
import {
  ServiceTroubleReported,
  errorKindOf,
  offersFor,
  troubleOf,
  troubleSentence,
} from "./serviceHelp";
import { SelectionStore } from "./selectionStore";
import { providerDisplayName, sessionTitle, workspaceName } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { ConversationItem, ProjectItem } from "./sidebarTargets";
import { StudioRuntimeClient } from "./runtimeClient";
import { sessionStateLabel } from "./runtimeProjection";
import type { ModelOption } from "./sessionConfiguration";
import { modelOptions, reasoningOptions, RECENT_SERVICE_KEY } from "./sessionConfiguration";
import { nativeTitleRefreshProviders } from "./nativeTitleRefresh";
import { nativeCatalogueAfterFailure } from "./nativeChatCatalogue";
import {
  projectNativeActivity,
  type NativeActivityProjection,
  unlistedLiveProviders,
} from "./nativeActivityProjection";
import {
  readStartDefaults,
  rememberStartDefault,
  START_DEFAULTS_KEY,
  type StartDefault,
} from "./startDefaults";
import {
  workingCollisions,
  workspaceCollisions,
  workspaceIdentity,
  type WorkspaceCollision,
} from "./workspaceCollision";

const NATIVE_ADOPTION_REFRESH_MS = 4 * 60_000;
/// Warning notices are one-shot hints, not a history store. Each owner keeps only its recent identities.
const WARNING_DEDUPE_CAPACITY = 64;
/// Existing conversations are discovered as soon as the surface is idle enough to ask.
///
/// Short on purpose. This delay only exists to let a foreground action finish first, never to stagger discovery
/// into a wait a person can watch.
const NATIVE_DISCOVERY_IDLE_MS = 150;

type NativeDiscovery = {
  abort: AbortController;
  pending: Promise<void>;
  force: boolean;
  queuedForce: Promise<void> | null;
};

/// Every way a caller can name the conversation it wants opened.
///
/// A tree row, a row of the list itself, a session record, or a bare session identifier. They all reduce to one
/// record before anything is opened, so there is exactly one path through selection.
export type SelectionTarget = ConversationItem | Conversation | SessionLine | string;

export class Controller implements vscode.Disposable {
  private indexAbort: AbortController | null = null;
  private readonly status: vscode.StatusBarItem;
  private selectionTail: Promise<void> = Promise.resolve();
  private selectionPersistenceTail: Promise<void> = Promise.resolve();
  private disposed = false;
  private readonly reportedRuntimeWarnings = new BoundedDedupe<string>(WARNING_DEDUPE_CAPACITY);
  private readonly reportedIsolatedWorkspaces = new BoundedDedupe<string>(WARNING_DEDUPE_CAPACITY);
  private readonly verifyingProviders = new Set<string>();
  private readonly nativeDiscoveries = new Map<string, NativeDiscovery>();
  private readonly capabilityDiscoveries = new Map<string, Promise<void>>();
  private readonly deferredNativeProviders = new Map<string, boolean>();
  /// The one terminal help commands are offered in, reused so repeated attempts do not stack up.
  private helpTerminal: vscode.Terminal | null = null;
  private nativeDiscoveryRestart: NodeJS.Timeout | null = null;
  private nativeDiscoveryPauseDepth = 0;
  /// The services this window has already asked for their stored conversations, so a service that
  /// becomes usable late is asked exactly once rather than on every listing the watch pushes.
  private readonly chatDiscoveryAsked = new Set<string>();
  private nativeDiscoveryGeneration = 0;
  /// One provider probe at a time. Each spawns a CLI, so the queue is what keeps activation answerable.
  private verificationTail: Promise<void> = Promise.resolve();
  private readonly isolatedWorkspaces: IsolatedWorkspaces;
  /// Native identity last seen for each pushed hosted terminal, used to refresh a catalogue only on discovery.
  private hostedTerminalIdentities = new Map<string, string | null>();
  /// Providers asked again for a live conversation no row lists, with the wait each one is on.
  private readonly unlistedReask = new Map<string, { members: string; askedAt: number; waitMs: number }>();
  private nativeActivityByProvider = new Map<string, ReadonlySet<string>>();
  private nativeAttachableByProvider = new Map<string, ReadonlySet<string>>();
  private nativeActiveByProvider = new Map<string, ReadonlySet<string>>();
  private nativeUnconfirmedByProvider = new Map<string, ReadonlySet<string>>();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: CoreClient,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly selection: SelectionStore,
    /// The projects the operator created, offered first when a draft picks its folder.
    private readonly projectRecords: { all(): readonly ProjectRecord[] },
    /// The conversation surface: one editor-area terminal tab per conversation, hosted by the Core.
    private readonly terminals: TerminalTabs,
  ) {
    this.isolatedWorkspaces = new IsolatedWorkspaces(
      client,
      () => runtime.integrationId(),
      () => runtime.reset(),
    );
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.name = "Runtrol";
    this.status.command = "runtrol.switchSession";
    this.status.show();
    context.subscriptions.push(
      this.status,
      state.onDidChange(() => this.updateStatus()),
      state.onDidChange((change) => {
        if (change !== "rows") return;
        void this.rememberKnownProjects();
        this.followRowIdentity();
      }),
    );
    this.state.setPinnedKeys(this.pinnedFromStorage());
    this.state.setRenamedTitles(this.renamedFromStorage());
  }

  /// The pinned set as this machine last remembered it.
  private pinnedFromStorage(): Set<string> {
    return new Set(this.context.globalState.get<string[]>(PINNED_CONVERSATIONS_KEY) ?? []);
  }

  /// The local nicknames this machine last remembered, one per conversation key. A nickname is Runtrol's own
  /// label for a conversation, never written back to the coding service: the service keeps owning its content,
  /// which is why renaming is instant and never has to wake the conversation up to change a stored name.
  private renamedFromStorage(): Map<string, string> {
    return new Map(Object.entries(
      this.context.globalState.get<Record<string, string>>(RENAMED_CONVERSATIONS_KEY) ?? {},
    ));
  }

  /// The conversation a sidebar action names, from the row it came from, the session it points at, or the
  /// selected one when a command was invoked with nothing in hand.
  private conversationFor(value?: ConversationItem | SessionLine): Conversation | null {
    if (value instanceof ConversationItem) return value.conversation;
    const sessionId = value?.sessionId ?? this.state.selected?.sessionId;
    return sessionId ? this.state.conversationOf(sessionId) : null;
  }

  /// Pin or unpin one conversation, remember it for next time, and repaint so it moves at once.
  async togglePin(value: ConversationItem | Conversation): Promise<void> {
    const row = value instanceof ConversationItem ? value.conversation : value;
    const pinned = this.pinnedFromStorage();
    // A conversation pinned before its service announced its own identity was remembered under the key it had
    // then. Both are cleared together, so unpinning actually unpins and no orphan is left to outlive the row.
    const wasPinned = pinned.delete(row.key) || (row.legacyKey !== null && pinned.delete(row.legacyKey));
    if (!wasPinned) {
      pinned.add(row.key);
    }
    await this.context.globalState.update(PINNED_CONVERSATIONS_KEY, [...pinned]);
    this.state.setPinnedKeys(pinned);
  }

  async initialize(): Promise<void> {
    const [inventory, remembered, isolated] = await Promise.all([
      this.runtime.inventory(),
      this.selection.load(),
      this.isolatedWorkspaces.list(),
    ]);
    this.state.setIsolatedWorkspaces(await this.reconcileIsolatedWorkspaces(
      isolated,
      inventory.sessions.sessions,
    ));
    this.applyListing(
      inventory.sessions.sessions,
      inventory.sessions.warnings,
      inventory.providers.providers,
    );
    const selected = this.state.sessions.find((session) => session.sessionId === remembered)
      ?? this.state.sessions.find((session) => session.hot)
      ?? null;
    if (selected) {
      // Restoring a highlight is not permission to start a provider process. In particular a remembered cold
      // conversation must not turn window activation into an implicit resume, and a hot conversation may belong
      // to a terminal host outside this window. The terminal index watch paints live ownership; an editor tab opens
      // only after an explicit row action.
      this.state.select(selected.sessionId);
    }
    this.startSessionIndexWatch();
    this.startProviderVerification(inventory.providers.providers);
    this.startExistingChatDiscovery();
  }

  async refresh(): Promise<void> {
    let inventory: Awaited<ReturnType<typeof this.runtime.inventory>>;
    let isolated: Awaited<ReturnType<typeof this.isolatedWorkspaces.list>>;
    try {
      [inventory, isolated] = await Promise.all([
        this.runtime.inventory(),
        this.isolatedWorkspaces.list(),
      ]);
    } catch (error) {
      // Say which of the two empty states this is. The tree's welcome asks the state for it, and without this
      // an unreachable Core reads as a machine with no coding service installed.
      this.state.setCoreReach("unreachable");
      throw error;
    }
    this.applyListing(
      inventory.sessions.sessions,
      inventory.sessions.warnings,
      inventory.providers.providers,
    );
    this.state.setIsolatedWorkspaces(await this.reconcileIsolatedWorkspaces(
      isolated,
      inventory.sessions.sessions,
    ));
    this.startProviderVerification(inventory.providers.providers);
    this.startExistingChatDiscovery();
  }

  private async refreshIsolatedWorkspaces(): Promise<void> {
    this.state.setIsolatedWorkspaces(await this.isolatedWorkspaces.list());
  }

  /// Resolve worktree ownership left across an Extension Host or Core restart. Unbound preparation is unused
  /// and may be removed only through Core's clean-only rule. A bound record whose Runtime session no longer
  /// exists is the same cleanup after a close from another surface. Dirty results stay and are named to the user.
  private async reconcileIsolatedWorkspaces(
    listed: readonly IsolatedWorkspaceLine[],
    sessions: readonly SessionLine[],
  ): Promise<readonly IsolatedWorkspaceLine[]> {
    const live = new Set(sessions.map((session) => session.sessionId));
    let changed = false;
    for (const workspace of listed) {
      if (workspace.state === "ready") {
        const opened = sessions.filter((session) => (
          workspaceIdentity(session.workspace) === workspaceIdentity(workspace.workspace)
        ));
        if (opened.length === 1 && opened[0]) {
          await this.isolatedWorkspaces.bind(workspace, opened[0].sessionId);
          changed = true;
          continue;
        }
      }
      const abandoned = workspace.state === "creating" || workspace.state === "ready";
      const closed = workspace.state === "bound"
        && workspace.session_id !== null
        && !live.has(workspace.session_id);
      if (!abandoned && !closed) continue;
      await this.isolatedWorkspaces.release(
        workspace.workspace,
        workspace.workspace_id,
        workspace.session_id,
      );
      changed = true;
    }
    const current = changed ? await this.isolatedWorkspaces.list() : listed;
    for (const workspace of current) {
      if (workspace.state !== "preservedDirty") continue;
      if (!this.reportedIsolatedWorkspaces.remember(workspace.workspace_id)) continue;
      void vscode.window.showWarningMessage(
        "Runtrol kept an isolated workspace because it contains changes.",
        { modal: false, detail: workspace.workspace },
        "Open worktree",
      ).then((action) => action === "Open worktree"
        ? vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(workspace.workspace), {
          forceNewWindow: true,
        })
        : undefined);
    }
    return current;
  }

  /// Bring a freshly widened workspace root into view without tearing anything down.
  ///
  /// The daemon re-reads this integration's grant from its store on every request and on every index
  /// publish, so a widened root is already in force on this very connection. Two things cannot arrive by
  /// themselves and are asked for here: one wider look at the index (the watch only pushes when sessions
  /// change, and the grant changing is not that), and the new folder's stored conversations. Measured
  /// before this path existed, the full reconnect this replaces put ~5 s between opening a folder and its
  /// conversations arriving.
  async refreshAfterRootWidened(): Promise<void> {
    this.cancelNativeDiscoveries();
    this.chatDiscoveryAsked.clear();
    this.state.clearNativeCatalogues();
    await this.refresh();
    this.startExistingChatDiscovery();
  }

  async refreshChats(): Promise<void> {
    // The visible list repaints from its live watch snapshots immediately. The explicit provider list request is
    // retained beside it because it is the zero-configuration trigger for a CLI installed since the last refresh;
    // Runtime performs that filesystem restamp behind the provider watch rather than on this response path.
    await Promise.all([this.refresh(), this.runtime.refreshProviderInventory()]);
    const providers = this.state.providers.filter(isUsable);
    await Promise.all(providers.flatMap((provider) => [
      this.loadNativeChats(provider.providerId, true),
      this.loadProviderCapabilities(provider.providerId),
    ]));
  }

  discoverNativeChats(providerId: string, force = false): void {
    if (this.nativeDiscoveryPauseDepth > 0 || this.nativeDiscoveryRestart) {
      this.deferNativeDiscovery(providerId, force);
      return;
    }
    void this.loadNativeChats(providerId, force);
  }

  /// Ask the Core where every declared service stands against its package registry.
  ///
  /// The Core asks the registry over the network for each service (up to thirty seconds each), and a command
  /// connection is serial: on the shared one, every other ask of this window waited behind it, which is how
  /// the refresh p95 went from tens of milliseconds to hundreds on CI (2026-08-29). The sidebar passes a
  /// connection of its own.
  async inspectProviderUpdates(channel: CoreClient = this.client): Promise<readonly ProviderUpdateLine[]> {
    const { response } = await channel.once({ ask: "providerUpdates" });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "providerUpdates") {
      throw new Error(`Core answered provider update inspection with ${response.say}`);
    }
    return response.with;
  }

  /// What the Core's last inspection said, without starting one: no package manager, no network, no lane.
  /// Empty before the Core's first inspection (a few minutes after it starts).
  async providerUpdateStatus(channel: CoreClient = this.client): Promise<readonly ProviderUpdateLine[]> {
    const { response } = await channel.once({ ask: "providerUpdateStatus" });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "providerUpdates") {
      throw new Error(`Core answered provider update status with ${response.say}`);
    }
    return response.with;
  }

  async checkProviderUpdates(): Promise<void> {
    const lines = await this.inspectProviderUpdates();
    const choices: Array<vscode.QuickPickItem & { update: ProviderUpdateLine }> = lines.map((line) => {
      const provider = this.state.providers.find((candidate) => candidate.providerId === line.provider);
      const label = provider?.displayName ?? line.provider;
      switch (line.state) {
        case "current":
          return {
            label,
            description: `Current ${line.installed ?? ""}`.trim(),
            detail: line.package ?? undefined,
            update: line,
          };
        case "available":
          return {
            label,
            description: `${line.installed ?? "unknown"} to ${line.target ?? "new release"}`,
            detail: line.rollback ? `Rollback available: ${line.rollback}` : "No earlier registry release",
            update: line,
          };
        case "observeOnly":
          return { label, description: "Provider-managed", detail: line.why ?? undefined, update: line };
        case "notInstalled":
          return { label, description: "Not installed", detail: line.why ?? undefined, update: line };
        case "unconfirmed":
          return { label, description: "Update channel unconfirmed", detail: line.why ?? undefined, update: line };
      }
    });
    const picked = await vscode.window.showQuickPick(choices, {
      title: "Provider update status",
      placeHolder: choices.length === 0 ? "No providers are declared" : "Select an available provider to update",
    });
    if (!picked || picked.update.state !== "available") {
      return;
    }
    if (!picked.update.rollback) {
      await vscode.window.showWarningMessage(
        `${picked.label} cannot be updated because no exact rollback release is available.`,
      );
      return;
    }
    const confirmed = await vscode.window.showWarningMessage(
      `Update ${picked.label} from ${picked.update.installed ?? "the installed release"} to ${picked.update.target ?? "the latest release"}?`,
      { modal: true },
      "Update provider",
    );
    if (confirmed !== "Update provider") {
      return;
    }
    await this.updateProvider(picked.update, picked.label);
  }

  /// Update one service to the release the Core inspected, and say how it went.
  ///
  /// No question here: the Update button already names the release it goes to, and the Core's transaction
  /// verifies the new release and rolls back to the exact earlier one on failure.
  async updateProvider(
    line: ProviderUpdateLine,
    label: string,
    /// The connection to run it on. An install takes minutes and a command connection is serial, so the
    /// sidebar hands a connection of its own here; on the shared one every other ask of this window would
    /// wait behind the install (closing a conversation, answering from a row, deleting).
    channel: CoreClient = this.client,
  ): Promise<void> {
    const result = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: `Updating ${label} to ${line.target ?? "the latest release"}` },
      async () => {
        const updated = await channel.once({
          ask: "providerUpdate",
          with: { provider: line.provider },
        });
        if (updated.response.say === "failed") {
          throw new Error(updated.response.with.message);
        }
        if (updated.response.say !== "providerUpdated") {
          throw new Error(`Core answered provider update with ${updated.response.say}`);
        }
        return updated.response.with;
      },
    );
    // The installed release changed under Runtime's probe; ask it again so the sidebar's version follows.
    await this.refresh();
    if (result.outcome === "updated") {
      const message = `${label} was updated from ${result.from} to ${result.to}.`;
      if (result.why) {
        await vscode.window.showWarningMessage(`${message} ${result.why}`);
      } else {
        await vscode.window.showInformationMessage(message);
      }
    } else if (result.outcome === "rolledBack") {
      await vscode.window.showWarningMessage(
        `${label} was restored to ${result.to} after update verification failed. ${result.why ?? ""}`.trim(),
      );
    } else {
      await vscode.window.showInformationMessage(`${label} is already current at ${result.to}.`);
    }
  }

  /// Go to the next conversation that has stopped for this person.
  ///
  /// One key, no scanning. With several agents running, the reader never has to work out which one wants them,
  /// and pressing it again walks the rest rather than returning to the same one.
  async openNextWaiting(): Promise<void> {
    const rows = this.state.conversations;
    const next = nextNeedingYou(rows, this.state.conversations.find((row) => row.open)?.key ?? null);
    if (!next) {
      // A transient line rather than a dialog. Being told nothing needs you should not itself need dismissing.
      this.context.subscriptions.push(
        vscode.window.setStatusBarMessage("Nothing is waiting for you.", 3_000),
      );
      return;
    }
    await this.selectConversation(next);
  }

  async switchSession(): Promise<void> {
    const rows = this.state.conversations;
    const picked = await vscode.window.showQuickPick(
      rows.map((conversation) => ({
        label: conversation.title,
        description: conversation.serviceName,
        detail: conversation.projectless ? undefined : conversation.folder,
        conversation,
      })),
      {
        title: `Conversations (${rows.length})`,
        placeHolder: "Type a project, service, or title",
        matchOnDescription: true,
        matchOnDetail: true,
      },
    );
    if (picked) {
      await this.selectConversation(picked.conversation);
    }
  }

  async reconnect(): Promise<void> {
    this.indexAbort?.abort();
    this.indexAbort = null;
    this.cancelNativeDiscoveries();
    this.chatDiscoveryAsked.clear();
    this.state.clearNativeCatalogues();
    await this.client.reset();
    await this.runtime.reset();
    await this.refreshAfterReconnect();
    // The catalogues were cleared above and the reconnect may exist precisely because the grant's roots grew
    // (a folder opened into a live window). Without restarting discovery here, stored conversations in the new
    // root stay invisible until someone happens to press refresh, which is the silent-forever failure again.
    this.startExistingChatDiscovery();
    void this.startSessionIndexWatch();
  }

  select(
    value: SelectionTarget,
    reveal = true,
  ): Promise<void> {
    const selected = this.selectionTail.then(() => this.selectNow(value, reveal));
    this.selectionTail = selected.catch(() => undefined);
    return selected;
  }

  private selectConversation(conversation: Conversation): Promise<void> {
    return this.select(conversation);
  }

  private async selectNow(
    value: SelectionTarget,
    reveal: boolean,
    afterApplied: () => void = () => undefined,
  ): Promise<void> {
    const target = this.resolve(value);
    const session = "key" in target ? target.session : target;
    if (session?.hot) {
      // A running conversation already has a Runtime identity and a bound provider. Native catalogue discovery
      // cannot change that selection, so aborting and restarting every provider listing here only competes with
      // the Webview and event-watch handshake the person is waiting for. Keep that background work intact.
      await this.applySelection(target, reveal, afterApplied);
      return;
    }
    const pausedDiscoveries = this.beginForegroundAction();
    try {
      await this.applySelection(target, reveal, afterApplied);
    } finally {
      this.endForegroundAction(pausedDiscoveries);
    }
  }

  /// Reduce every way a conversation can be named down to the one record that opens it.
  private resolve(value: SelectionTarget): Conversation | SessionLine {
    if (value instanceof ConversationItem) return value.conversation;
    if (typeof value !== "string" && "key" in value) return value;
    const id = typeof value === "string" ? value : value.sessionId;
    const session = this.state.sessions.find((candidate) => candidate.sessionId === id);
    if (!session) throw new Error("that conversation is no longer listed");
    return session;
  }

  /// Say why a running conversation will not open here, and offer the folder's next conversation.
  ///
  /// One button and no modal: the person clicked a row, so the answer belongs where they are looking, and the
  /// only thing Runtrol can actually do for them is start another conversation in the same folder.
  private async offerAnotherHere(row: Conversation): Promise<void> {
    const start = "Start a conversation here";
    // A conversation living in the service's own editor panel has no terminal to join, but the panel itself
    // is right here in this editor: the click can put the person in front of the real surface (operator,
    // 2026-08-30: the editor's Claude panel session, resumed as a terminal, showed a frozen copy instead).
    const panel = row.native
      ? editorPanelFor(row.providerId, (extension) => vscode.extensions.getExtension(extension) !== undefined)
      : null;
    const revealLabel = `Open in ${row.serviceName}`;
    // The row already carries the sentence, so the message says what the tooltip says and cannot drift from it.
    const why = row.blocked ?? `${row.title} cannot be opened here.`;
    // Buttons are offered only where they lead somewhere: a conversation with no folder has nowhere to start.
    const offered = [
      ...(panel ? [revealLabel] : []),
      ...(row.workspace ? [start] : []),
    ];
    const said = await vscode.window.showInformationMessage(why, ...offered);
    if (said === revealLabel && panel && row.native) {
      await vscode.commands.executeCommand(panel.reveal, row.native.nativeSessionId);
      return;
    }
    if (said !== start || !row.workspace) return;
    await this.startSessionInWorkspace(row.workspace);
  }

  /// What the Runtime answered to the last owner reveal this window asked for, for the journey to read.
  lastReveal: WindowRevealResult | null = null;

  private async applySelection(
    value: SelectionTarget,
    reveal: boolean,
    afterApplied: () => void,
  ): Promise<void> {
    const target = this.resolve(value);
    // A conversation row opens the service's own terminal interface in an editor tab. That is the whole
    // surface (`docs/terminalSurface.md`): no adoption, no structured session, no page of ours.
    if ("key" in target) {
      // A terminal another window owns is shown there: the owner is asked to show it and its window is brought
      // forward as far as Windows permits (`docs/vscodeSurface.md`, owner reveal). Nothing opens here.
      const owner = target.hostedTerminal?.ownerWindowSessionId ?? null;
      const ownerKey = target.hostedTerminal?.ownerTerminalKey ?? null;
      if (owner !== null && ownerKey !== null && owner !== vscode.env.sessionId) {
        const outcome = await this.runtime.revealAtOwner({ windowSessionId: owner, terminalKey: ownerKey });
        this.lastReveal = outcome;
        this.say(
          outcome.delivered
            ? `Shown in its own window (${outcome.foreground})`
            : `Its window is not listening for that (${outcome.foreground})`,
          outcome.delivered ? "info" : "warning",
        );
        afterApplied();
        return;
      }
      // A conversation running in a terminal another window owns is shown there, not opened here: the same
      // desktop answer as a hosted row owned elsewhere, for the tier that has no mirror at all.
      if (target.canFocus && target.native?.nativeSessionId) {
        const outcome = await this.runtime.focusNative({
          providerId: target.providerId,
          nativeSessionId: target.native.nativeSessionId,
        });
        this.lastReveal = outcome;
        this.say(
          outcome.delivered
            ? `Shown in the window that runs it (${outcome.foreground})`
            : `Its window is not listening for that (${outcome.foreground})`,
          outcome.delivered ? "info" : "warning",
        );
        afterApplied();
        return;
      }
      if (!target.canOpen) {
        // A live conversation without a currently proven terminal route is the one refusal a person meets every
        // day on a machine that also runs CLIs directly or still has a legacy Runtime owner. An error toast in
        // protocol words leaves them nowhere, so the click offers the thing that does work.
        if (runningElsewhere(target)) {
          await this.offerAnotherHere(target);
          afterApplied();
          return;
        }
        throw new Error(target.blocked ?? "that conversation cannot be opened");
      }
      this.terminals.show(target, !reveal);
      if (target.session) this.state.select(target.session.sessionId);
      afterApplied();
      return;
    }
    // A session named directly (a restored selection): its row's terminal, when the row is
    // listed. Deliberately no window-follow here: selecting a conversation opens ITS tab beside whatever is
    // already open and NOTHING else moves (`docs/vscodeSurface.md`).
    const session: SessionLine = target;
    const row = this.state.conversations.find((candidate) => candidate.session?.sessionId === session.sessionId);
    if (row?.canOpen) this.terminals.show(row, !reveal);
    const stored = this.persistSelection(session.sessionId);
    this.state.select(session.sessionId);
    void stored.catch((error: unknown) => {
      this.say(`Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`, "warning");
    });
    afterApplied();
  }

  /// A word to the person about what just happened. Information goes to the status bar for a moment;
  /// anything worse is a notification, because it may need an answer.
  private say(message: string, kind: "info" | "warning" | "error" = "info"): void {
    if (kind === "info") {
      vscode.window.setStatusBarMessage(message, 4_000);
    } else if (kind === "warning") {
      void vscode.window.showWarningMessage(message);
    } else {
      void vscode.window.showErrorMessage(message);
    }
  }

  /// Text into a structured session on the public Runtime. The journey harness speaks that protocol; the
  /// terminal surface is what people use.
  async submitResolvedInput(sessionId: string, text: string): Promise<void> {
    await this.refresh();
    const session = this.state.sessions.find((candidate) => candidate.sessionId === sessionId);
    if (!session) {
      throw new Error("that session is no longer listed by Runtime");
    }
    await this.runtime.submitInput(runtimeAction(session), text);
  }

  /// A structured session's model, set on the public Runtime (the journey harness; a person uses the
  /// service's own `/model` in the terminal).
  async setSelectedModel(session: SessionLine, model: string): Promise<void> {
    await this.runtime.setModel(runtimeAction(session), model);
  }

  /// A structured session's mode, likewise.
  async setSelectedMode(session: SessionLine, mode: string): Promise<void> {
    await this.runtime.setMode(runtimeAction(session), mode);
  }

  /// Spread the open conversation tabs over a grid of editor groups; one command, one screen of agents.
  async arrangeConversationGrid(): Promise<void> {
    const arranged = await this.terminals.arrangeGrid();
    vscode.window.setStatusBarMessage(
      arranged === 0
        ? "Open a conversation or two first; the grid arranges the conversation tabs that are open."
        : `${arranged} conversations arranged in a grid.`,
      4_000,
    );
  }

  /// New conversation: pick the service, and its own terminal interface opens in this window's folder (or
  /// the scratch folder when the window has none). The service creates the conversation on its first turn,
  /// with its own composer, model picker and permission prompts; nothing of ours stands in front of it.
  /// The Conversations section's own button: a conversation that belongs to no project.
  ///
  /// Deliberately not the folder this window has open. The panel is the machine's, not this window's
  /// (`docs/vscodeSurface.md`), and a conversation filed under a folder nobody added would vanish from the
  /// list the moment somebody looked from another window. Starting one inside a project is the project row's
  /// own button.
  async startSession(options: { interactive?: boolean } = {}): Promise<void> {
    await this.startSessionInWorkspace(await this.ensureProjectlessRoot(), options);
  }

  /// New conversation inside one project, from its heading: the folder question already answered.
  ///
  /// `interactive: false` never asks: the service used last, or the first usable one, opens. For callers
  /// with nobody at the keyboard (a journey, an automation), where a picker would wait forever.
  async startSessionInWorkspace(workspace: string, options: { interactive?: boolean } = {}): Promise<void> {
    const usable = this.state.providers.filter((provider) => isUsable(provider));
    if (usable.length === 0) {
      // Not an error: a machine with no service yet is a normal first day, and the Agent Usage view says
      // how to add one. Said in a notification and nothing opens.
      this.say("No coding service is installed and signed in yet. Add one from the Agent Usage view.", "warning");
      return;
    }
    if (options.interactive === false) {
      await this.startSessionWith(this.orderedServices(usable)[0]!.providerId, workspace);
      return;
    }
    // An interactive start always names the available service, even when there is only one. Auto-opening the
    // sole usable entry made a broken terminal launch look like an empty provider choice: the person pressed
    // Add, saw no provider and no tab, and had no second action to identify which side failed. The choice is
    // offered where they pressed the button (`docs/vscodeSurface.md`).
    this.chooseService?.(workspace);
  }

  /// Where the sidebar draws the service choice, set by the surface that owns the sections.
  chooseService: ((workspace: string) => void) | null = null;

  /// Start a conversation with one named service, which is what a chosen row asks for.
  async startSessionWith(providerId: string, workspace: string): Promise<void> {
    await this.context.globalState.update(RECENT_SERVICE_KEY, providerId);
    // Until the service names the conversation, the tab says what the person just did. It used to say the folder,
    // which for a conversation with no project read `no-project`: the one name on screen was the name of an
    // implementation detail (measured 2026-08-26).
    const projectless = isProjectless(workspace, this.state.projectlessRoot);
    const name = projectless
      ? `New ${providerDisplayName(providerId, this.state.providers)} conversation`
      : workspaceName(workspace) || workspace;
    this.terminals.showFresh(providerId, workspace, name, projectless);
  }

  /// The services a person may start, most recently used first.
  private orderedServices(usable: readonly ProviderLine[]): ProviderLine[] {
    const recent = this.context.globalState.get<string>(RECENT_SERVICE_KEY) ?? null;
    return [...usable].sort(
      (left, right) => Number(right.providerId === recent) - Number(left.providerId === recent),
    );
  }

  /// The services the sidebar offers in its own chooser, most recently used first.
  startableServices(): ProviderLine[] {
    return this.orderedServices(this.state.providers.filter((provider) => isUsable(provider)));
  }


  /// The scratch folder, created on first use. One `mkdir` of an empty directory; nothing is written in it
  /// by this extension (the coding CLI runs there, exactly as it runs in any project folder).
  private async ensureProjectlessRoot(): Promise<string> {
    const root = this.state.projectlessRoot;
    if (!root) throw new Error("this window has no folder for conversations without a project");
    await mkdir(root, { recursive: true });
    return root;
  }

  async startResolvedSession(
    providerId: string,
    workspace: string,
    model: string | null,
    reasoningEffort: string | null,
    access: StartDecision,
    follow: boolean,
    permission: string | null = null,
  ): Promise<string> {
    const provider = this.state.providers.find((candidate) => candidate.providerId === providerId);
    if (!provider) {
      throw new Error(`the installed coding service ${providerId} is no longer listed`);
    }
    // A service that cannot run is the commonest reason a conversation never starts, and it is the one a
    // person can fix in a minute if anybody tells them how. This used to be a bare sentence about
    // usability, which is true and useless.
    if (!isUsable(provider)) {
      return await this.reportServiceTrouble(provider, troubleOf(undefined, provider));
    }
    const pausedDiscoveries = this.beginForegroundAction();
    try {
      // A structured session on the public Runtime, the shape the journey harness speaks. The
      // conversation surface for people is the terminal tab; this path is for the machinery.
      const openedId = (await this.runtime.start(
        provider.providerId,
        workspace,
        access,
        model,
        reasoningEffort,
        permission,
      )).sessionId;
      await this.refresh();
      await this.select(openedId);
      return openedId;
    } catch (error) {
      if (error instanceof ServiceTroubleReported) throw error;
      const trouble = troubleOf(errorKindOf(error), provider);
      // An unrecognised failure is not the coding service's fault as far as anybody knows, so it keeps the
      // original error rather than being dressed up as a service problem with actions that cannot help.
      if (trouble === "unknown" && errorKindOf(error) === undefined) throw error;
      return await this.reportServiceTrouble(provider, trouble);
    } finally {
      this.endForegroundAction(pausedDiscoveries);
    }
  }

  /// Keep the open tabs filed under the rows they are: a placeholder the service has just named hands its tab to
  /// the conversation row, and a hosted terminal row that gained its conversation identity does the same. The list
  /// already made these judgements (`namedPlaceholders`, the hosted claim); the tabs must follow them in the same
  /// moment, or the row a person just watched get its name opens a second tab on the next click (measured
  /// 2026-09-05: the named row read as not open while its tab kept the folder's name).
  private followRowIdentity(): void {
    const rows = this.state.conversations;
    this.terminals.retire(namedPlaceholders(rows, this.terminals.startedConversations()));
    this.terminals.reconcileHosted(rows);
  }

  /// Say what a coding service could not do, and offer that service's own commands for fixing it.
  ///
  /// Buttons rather than a sentence naming a command, because a person who has to retype a command from a
  /// toast has been told about the fix rather than helped with it. The chosen line goes into their own
  /// terminal unexecuted.
  ///
  /// Always throws. The caller has a signature that promises a session, and there is no session; the
  /// distinct error type is what stops the command wrapper from stacking a second message on top of this
  /// one.
  private async reportServiceTrouble(
    provider: ProviderLine,
    trouble: ServiceTrouble,
  ): Promise<never> {
    const sentence = troubleSentence(provider, trouble);
    const offers = offersFor(provider, trouble);
    if (offers.length === 0) {
      // Nothing declared to offer. Still say what happened in the person's own terms rather than falling
      // through to a protocol string.
      await vscode.window.showErrorMessage(sentence);
      throw new ServiceTroubleReported(sentence);
    }
    const chosen = await vscode.window.showErrorMessage(
      sentence,
      ...offers.map((offer) => offer.label),
    );
    const offer = offers.find((candidate) => candidate.label === chosen);
    if (offer) {
      this.offerInTerminal(offer);
    }
    throw new ServiceTroubleReported(sentence);
  }

  /// Offer a struggling service's own remedies from its row, before any conversation has failed.
  ///
  /// The same vocabulary the start-failure dialog uses (`troubleOf` + `offersFor`), reachable from the
  /// sidebar's fixed CLI status row so a person does not have to attempt a conversation just to be told how to fix
  /// the service. The chosen line lands in their terminal unexecuted, exactly like every other offer.
  /// Set one coding service up, from the usage strip's set-up list.
  ///
  /// One press, one outcome, decided from what the service itself reported: a service nobody has signed into
  /// gets its sign-in command, one that is not installed gets its install command, and one that is installed but
  /// cannot start goes to the fuller repair surface because there its trouble has more than one answer. Both
  /// commands are placed in a terminal and left unrun, which is the boundary this product has always held.
  async setUpService(provider: ProviderLine): Promise<void> {
    if (provider.account?.status === "signedOut") {
      this.signInProvider(provider);
      return;
    }
    if (provider.installation.state === "missing") {
      const install = offersFor(provider, "notInstalled").find((offer) => offer.command === provider.help?.install) ?? null;
      if (!install) {
        this.say(`${provider.displayName} declares no install command; install it from its own site.`, "info");
        return;
      }
      this.offerInTerminal(install);
      return;
    }
    await this.fixService(provider);
  }

  async fixService(provider: ProviderLine): Promise<void> {
    const trouble = troubleOf(undefined, provider);
    const offers = offersFor(provider, trouble);
    if (offers.length === 0) {
      void vscode.window.showInformationMessage(
        provider.installation.why
          ?? `${provider.displayName} cannot currently start a conversation, and it declares no help commands.`,
      );
      return;
    }
    const picked = await vscode.window.showQuickPick(
      offers.map((offer) => ({ label: offer.label, detail: offer.because, offer })),
      {
        title: `${provider.displayName} needs attention`,
        placeHolder: "The chosen command is placed in your terminal, never run for you",
      },
    );
    if (!picked) return;
    this.offerInTerminal(picked.offer);
  }

  /// Put a coding service's own command in the operator's terminal, running it or leaving it for them.
  ///
  /// `run` is the line between two different things a person means by pressing a button here. An install
  /// command (`npm i -g …`) is Runtrol fetching and executing on somebody's behalf, the one capability this
  /// product refused from the start, so it is placed and left for the person to read and run. A sign-in is
  /// the opposite: the provider CLI authenticates itself through its own browser flow, holding its own
  /// credential, which is exactly the thin boundary this product keeps. Leaving `claude auth login` typed but
  /// unrun did not honour that boundary, it just made sign-in not work (operator, 2026-08-29: pressing sign
  /// in only wrote to the terminal and no login opened). So a sign-in runs to the end, and the person
  /// completes it in the browser the CLI opens.
  private offerInTerminal(offer: HelpOffer, run = false): void {
    if (this.helpTerminal?.exitStatus !== undefined) {
      this.helpTerminal = null;
    }
    this.helpTerminal ??= vscode.window.createTerminal({ name: "Runtrol: coding service" });
    this.helpTerminal.show(true);
    this.helpTerminal.sendText(offer.command, run);
  }

  async interrupt(from?: SessionLine): Promise<void> {
    const session = from ?? this.requireSelected();
    await this.runtime.interrupt(runtimeAction(session));
  }

  async openConversation(): Promise<void> {
    const selected = this.state.selected;
    if (!selected) return;
    // The selected conversation's terminal tab, brought to the front. A selection with no row yet (the
    // index is still arriving) has nothing to show, and showing nothing is the honest answer.
    const row = this.state.conversations.find((candidate) => candidate.session?.sessionId === selected.sessionId);
    if (row) this.terminals.show(row, false);
  }

  /// Move this window to another project, remembering where it was so one key brings it back.
  ///
  /// The only way the window changes what it is open on (the contract: opening a conversation never moves the
  /// window). The previous target is one string in global state, read by the window that replaces this one.
  async switchWindowTo(workspace: string): Promise<void> {
    const current = currentWindowTarget();
    if (current) await this.context.globalState.update(PREVIOUS_PROJECT_KEY, current);
    await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(workspace), { forceReuseWindow: true });
  }

  /// Move this window to one of the machine's projects from the keyboard (`Ctrl+K Ctrl+Shift+P`): every
  /// project heading the sidebar shows, plus this window's folders, in one list; the same switch the heading's
  /// button makes, reachable without the mouse.
  async switchProject(): Promise<void> {
    type Choice = vscode.QuickPickItem & { workspace: string };
    const projectChoices = (): Choice[] => {
      const seen = new Set<string>();
      const choices: Choice[] = [];
      const add = (workspace: string, description: string): void => {
        const identity = workspaceIdentity(workspace);
        if (seen.has(identity)) return;
        seen.add(identity);
        choices.push({ label: workspaceName(workspace) || workspace, description, detail: workspace, workspace });
      };
      const openFolders = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
      for (const group of projects(this.projectRecords.all(), this.state.conversations, openFolders)) {
        add(group.workspace, group.current ? "This window" : `${group.rows.length} conversation${group.rows.length === 1 ? "" : "s"}`);
      }
      for (const folder of vscode.workspace.workspaceFolders ?? []) add(folder.uri.fsPath, "This window");
      // Projects any window of this machine has listed before: a fresh window's own list is still loading,
      // and the keyboard should not wait for it.
      for (const workspace of this.context.globalState.get<string[]>(KNOWN_PROJECTS_KEY) ?? []) add(workspace, "Seen before");
      return choices;
    };
    // A live list: a fresh window is still discovering the machine's conversations while the picker is open,
    // so the list fills in as they arrive instead of asking the person to close and reopen it.
    const picker = vscode.window.createQuickPick<Choice>();
    picker.title = "Switch this window to a project";
    picker.placeholder = "The window moves there; Ctrl+K Ctrl+B brings it back";
    picker.matchOnDescription = true;
    picker.matchOnDetail = true;
    picker.items = projectChoices();
    picker.busy = this.discoveringNativeChats();
    const follow = this.state.onDidChange((change) => {
      if (change !== "rows") return;
      const active = picker.activeItems[0]?.workspace ?? null;
      picker.items = projectChoices();
      picker.busy = this.discoveringNativeChats();
      const again = active ? picker.items.find((item) => item.workspace === active) : undefined;
      if (again) picker.activeItems = [again];
    });
    const picked = await new Promise<Choice | undefined>((resolve) => {
      picker.onDidAccept(() => resolve(picker.selectedItems[0]));
      picker.onDidHide(() => resolve(undefined));
      picker.show();
    });
    follow.dispose();
    picker.dispose();
    if (!picked) return;
    if (workspaceIsOpen(picked.workspace)) {
      this.say(`${picked.label} is already this window.`, "info");
      return;
    }
    await this.switchWindowTo(picked.workspace);
  }

  /// Whether any service's stored conversations are still being listed (the picker says so with a spinner).
  private discoveringNativeChats(): boolean {
    return this.state.providers.filter(isUsable).some((provider) => this.state.nativeCatalogue(provider.providerId) === null);
  }

  /// The remembered project folders, for the harness.
  knownProjectsForJourney(): readonly string[] {
    return this.context.globalState.get<string[]>(KNOWN_PROJECTS_KEY) ?? [];
  }

  async isolatedWorkspaceEvidenceForJourney(): Promise<{
    workspaces: readonly IsolatedWorkspaceLine[];
    roots: readonly string[];
  }> {
    const [workspaces, roots] = await Promise.all([
      this.isolatedWorkspaces.list(),
      this.isolatedWorkspaces.authorizedRoots(),
    ]);
    return { workspaces, roots };
  }

  /// Remember the projects the sidebar shows, so the keyboard switch can offer them in a window that has not
  /// listed anything yet. Paths only, bounded, newest last.
  private async rememberKnownProjects(): Promise<void> {
    const known = this.context.globalState.get<string[]>(KNOWN_PROJECTS_KEY) ?? [];
    const seen = new Set(known.map((workspace) => workspaceIdentity(workspace)));
    const next = [...known];
    const openFolders = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
    for (const group of projects(this.projectRecords.all(), this.state.conversations, openFolders)) {
      const identity = workspaceIdentity(group.workspace);
      if (seen.has(identity)) continue;
      seen.add(identity);
      next.push(group.workspace);
    }
    if (next.length === known.length) return;
    await this.context.globalState.update(KNOWN_PROJECTS_KEY, next.slice(-MAX_KNOWN_PROJECTS));
  }

  /// Back to the project this window was on before the last switch (`Ctrl+K Ctrl+B`), in the same window.
  async returnToPreviousProject(): Promise<void> {
    const previous = this.context.globalState.get<string>(PREVIOUS_PROJECT_KEY) ?? null;
    const current = currentWindowTarget();
    if (!previous || previous === current) {
      this.say("No previous project to return to: this window has not moved yet.", "info");
      return;
    }
    if (current) await this.context.globalState.update(PREVIOUS_PROJECT_KEY, current);
    await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.parse(previous), { forceReuseWindow: true });
  }

  /// The service's own sign-in line, placed in the operator's terminal from the row that says it is needed.
  async signInFromRow(value?: ConversationItem | SessionLine): Promise<void> {
    const session = this.sessionOf(value);
    const provider = this.state.providers.find((candidate) => candidate.providerId === session.providerId) ?? null;
    if (!provider) {
      this.say(`${providerDisplayName(session.providerId, this.state.providers)} is not listed.`, "info");
      return;
    }
    this.signInProvider(provider);
  }

  /// The service's own sign-in command, typed into a terminal and left there: Runtrol never runs a login
  /// itself and never holds what one produces. Reachable from a conversation row and from the usage strip.
  signInProvider(provider: ProviderLine): void {
    const signIn = offersFor(provider, "needsSigningIn").find((offer) => offer.command === provider.help?.signIn) ?? null;
    if (!signIn) {
      this.say(`${provider.displayName} declares no sign-in command; sign in at its own surface.`, "info");
      return;
    }
    // Run it: the CLI opens its own browser flow and the person finishes there. Runtrol holds nothing.
    this.offerInTerminal(signIn, true);
  }

  /// What the Core's private help line says about one service: the sign-out command, when it declares one.
  ///
  /// The private admin answer rather than the public inventory, because the public inventory is validated
  /// against a closed schema by every shipped window and a new field there breaks them (2026-08-29).
  async providerHelpLine(
    providerId: string,
    channel: CoreClient = this.client,
  ): Promise<{ signOut: string | null } | null> {
    const { response } = await channel.once({ ask: "providerHelp", with: { provider_id: providerId } });
    if (response.say !== "providerHelp") return null;
    return { signOut: response.with.sign_out ?? null };
  }

  /// The service's own sign-out command, run in a terminal the same way sign-in is: the CLI clears what it
  /// stored, and Runtrol holds nothing to clear. Reachable from the usage panel of a signed-in account.
  async signOutProvider(provider: ProviderLine): Promise<void> {
    const command = (await this.providerHelpLine(provider.providerId))?.signOut ?? null;
    if (!command) {
      this.say(`${provider.displayName} declares no sign-out command; sign out at its own surface.`, "info");
      return;
    }
    this.offerInTerminal({
      label: `Sign out of ${provider.displayName}`,
      command,
      because: "this coding service keeps its own login, and its own command is what ends it",
    }, true);
  }

  /// Answer the question a conversation is waiting on from its row, with the service's own options, without
  /// opening the page. "allow" and "decline" take the first option of that kind; "choose" lists them all.
  async answerFromRow(value: ConversationItem | SessionLine | undefined, how: "allow" | "decline" | "choose"): Promise<void> {
    const session = this.sessionOf(value);
    const pending = await this.runtime.listPendingApprovals(runtimeAction(session));
    const approval = pending[0];
    if (!approval) {
      this.say(`${sessionTitle(session)} is not waiting on a question right now.`, "info");
      await this.refresh();
      return;
    }
    const usable = approval.options.filter((option) => option.unavailable == null);
    let option = null as (typeof usable)[number] | null;
    if (how === "choose") {
      const picked = await vscode.window.showQuickPick(
        usable.map((candidate) => ({ label: candidate.label, description: candidate.kind, candidate })),
        { title: `${sessionTitle(session)}: ${approval.kind}`, placeHolder: "The service's own options, answered from the sidebar" },
      );
      option = picked?.candidate ?? null;
    } else {
      const wanted = how === "allow" ? ["allowOnce", "allowAlways"] : ["rejectOnce", "rejectAlways"];
      option = usable.find((candidate) => wanted.includes(candidate.kind)) ?? null;
      if (!option) {
        this.say(`${sessionTitle(session)} offers no ${how} option for this question; choose from its own list instead.`, "info");
        return;
      }
    }
    if (!option) return;
    await this.runtime.answerApproval(runtimeAction(session), approval.approvalId, option.optionId, approval.subjectDigest);
    await this.refresh();
  }

  /// What the back key would return to, for the harness.
  previousProjectForJourney(): string | null {
    return this.context.globalState.get<string>(PREVIOUS_PROJECT_KEY) ?? null;
  }

  async nameSession(value?: ConversationItem | SessionLine): Promise<void> {
    const row = this.conversationFor(value);
    if (!row) return;
    const chosen = await vscode.window.showInputBox({
      title: `Rename ${row.title}`,
      prompt: "Use a short name for this chat. Leave it empty to restore the service's name.",
      value: this.renamedFromStorage().get(row.key) ?? "",
      ignoreFocusOut: true,
      validateInput: validateSessionLabel,
    });
    if (chosen === undefined) return;
    await this.renameConversation(row, chosen);
  }

  /// Set or clear a conversation's local nickname by key, then repaint so the new name shows at once. An empty
  /// name restores the coding service's own title. The nickname is keyed on the conversation, not the running
  /// session, so it survives the conversation being opened, closed and reopened.
  async renameConversation(row: Conversation, label: string): Promise<void> {
    const names = this.renamedFromStorage();
    const normalized = label.trim();
    // The name is written under the key the row carries now, and the one it carried before its service named
    // the conversation is dropped either way, so a cleared name cannot come back from the older entry.
    if (row.legacyKey !== null) names.delete(row.legacyKey);
    if (normalized) {
      names.set(row.key, normalized);
    } else {
      names.delete(row.key);
    }
    await this.context.globalState.update(RENAMED_CONVERSATIONS_KEY, Object.fromEntries(names));
    this.state.setRenamedTitles(names);
  }

  async answerApproval(approval: string, option: number, subjectDigest: number[], from?: SessionLine): Promise<void> {
    const session = from ?? this.requireSelected();
    await this.runtime.answerApproval(
      runtimeAction(session),
      approval,
      option,
      subjectDigest,
    );
  }

  /// Delete a conversation from its sidebar action.
  async deleteConversation(value: ConversationItem | Conversation): Promise<void> {
    const row = value instanceof ConversationItem ? value.conversation : value;
    const capabilities = await this.capabilitiesFor(row.providerId);
    const decision = conversationDeletion(row, capabilities);
    if (decision.kind === "unsupported") {
      void vscode.window.showInformationMessage(decision.why);
      return;
    }
    const question = deletionQuestion(row, decision.serviceName);
    const choice = await vscode.window.showWarningMessage(
      question.message,
      { modal: true, detail: question.detail },
      question.button,
    );
    if (choice !== question.button) return;
    const title = row.title;
    const serviceName = row.serviceName;
    await this.deleteNativeWithoutAsking(row);
    // The row simply disappears otherwise, which is the same thing a misclick looks like. Naming what left and
    // whose list it left says which of the two just happened. It claims nothing about getting it back, because
    // that is each service's own business and not the same answer for all of them.
    void vscode.window.showInformationMessage(`Deleted ${title} from ${serviceName}.`);
  }

  /// Delete every conversation of one project that its service can delete, after one confirmation carrying
  /// the exact numbers (`projectDeletion.ts`). Reached from the project row's context menu, never from a
  /// hover icon: a misclick beside "new conversation" must not be able to do this (operator, 2026-08-29).
  async deleteProjectConversations(item: ProjectItem): Promise<void> {
    const rows = item.group.rows;
    const capabilities = new Map<string, ProviderCapabilities | null>();
    for (const providerId of new Set(rows.map((row) => row.providerId))) {
      capabilities.set(providerId, await this.capabilitiesFor(providerId));
    }
    const plan = planProjectDeletion(rows, (providerId) => capabilities.get(providerId) ?? null);
    const question = projectDeletionQuestion(item.group.name, plan);
    if (!question) {
      const kept = [...plan.undeletable]
        .map(([service, count]) => `${count} of ${service} (no deletion published)`)
        .join(", ");
      const elsewhere = plan.runningElsewhere.length > 0
        ? `${plan.runningElsewhere.length} running outside Runtrol`
        : "";
      const reasons = [kept, elsewhere].filter((reason) => reason !== "").join("; ");
      void vscode.window.showInformationMessage(
        `Nothing in ${item.group.name} can be deleted right now${reasons ? `: ${reasons}` : ""}.`,
      );
      return;
    }
    // The idle-only button first: the first button is what Enter presses, and Enter must not stop agents.
    const buttons = [question.deleteIdle, question.stopAndDelete]
      .filter((label): label is string => label !== null);
    const choice = await vscode.window.showWarningMessage(
      question.message,
      { modal: true, detail: question.detail },
      ...buttons,
    );
    if (!choice) return;
    const stopping = choice === question.stopAndDelete ? plan.stoppable : [];
    const intended = plan.deletable.length + stopping.length;
    let deleted = 0;
    const refused: string[] = [];
    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Deleting ${intended} conversations from ${item.group.name}`,
      },
      async (progress) => {
        // A stop answers before the process has ended, and the Core keeps the conversation's live claim
        // until it has seen the exit (a settle of a few hundred milliseconds). Deleting inside that window
        // is refused as "stop its original session first", which is nonsense to a person who just pressed
        // Stop. So each stopped row is deleted only once this window's own row for it has gone idle, which
        // is the Core saying the claim is gone; one that does not go idle in time is kept and said.
        const stopped: Conversation[] = [];
        for (const row of stopping) {
          progress.report({ message: `stopping ${row.title}` });
          try {
            if (row.presence.kind === "hosted") {
              await this.runtime.stopTerminal(row.presence.terminal);
              if (!(await this.awaitIdle(row.key, STOP_SETTLE_MS))) {
                refused.push(`${row.title}: still running after Stop, so it was kept`);
                continue;
              }
            }
            // A supervised session is closed by the deletion itself, which knows how (deleteNativeWithoutAsking).
            stopped.push(row);
          } catch (error) {
            refused.push(`${row.title}: ${error instanceof Error ? error.message : String(error)}`);
          }
        }
        for (const row of [...plan.deletable, ...stopped]) {
          progress.report({ message: row.title });
          try {
            await this.deleteNativeWithoutAsking({ ...row, live: false });
            deleted += 1;
          } catch (error) {
            refused.push(`${row.title}: ${error instanceof Error ? error.message : String(error)}`);
          }
        }
      },
    );
    await this.refreshChats();
    const kept = [...plan.undeletable].map(([service, count]) => `${count} (${service} cannot delete)`);
    if (plan.runningElsewhere.length > 0) kept.push(`${plan.runningElsewhere.length} running outside Runtrol`);
    const tail = [
      kept.length > 0 ? `Kept: ${kept.join(", ")}.` : "",
      refused.length > 0 ? `Refused: ${refused.join("; ")}` : "",
    ].filter((line) => line !== "").join(" ");
    const summary = `Deleted ${deleted} of ${intended} from ${item.group.name}.${tail ? ` ${tail}` : ""}`;
    if (refused.length > 0) void vscode.window.showWarningMessage(summary);
    else void vscode.window.showInformationMessage(summary);
  }

  /// Whether this window's row for a conversation stops being live within the deadline: the Core's own word,
  /// through the terminal index it pushes, that the process ended and the conversation's claim went with it.
  private awaitIdle(key: string, deadlineMs: number): Promise<boolean> {
    const idle = (): boolean => {
      const row = this.state.conversations.find((candidate) => candidate.key === key);
      return !row || !row.live;
    };
    if (idle()) return Promise.resolve(true);
    return new Promise((resolve) => {
      const settle = (answer: boolean): void => {
        clearTimeout(timer);
        watching.dispose();
        resolve(answer);
      };
      const timer = setTimeout(() => settle(idle()), deadlineMs);
      const watching = this.state.onDidChange(() => {
        if (idle()) settle(true);
      });
    });
  }

  /// The deletion itself, after the question (or, for the headless journey, instead of it).
  ///
  /// The row leaves the sidebar on the click, before the provider is asked: the wait is the provider's,
  /// not the person's, and a row that lingered for a full store rescan read as a deletion that had not
  /// happened (measured 2026-08-25: the rescan re-read every transcript of the service). The provider's
  /// answer is still the word on what exists: a refusal puts the row back, with the refusal beside it.
  async deleteNativeWithoutAsking(row: Conversation): Promise<void> {
    const native = row.native;
    if (!native) throw new Error(`${row.title} has nothing left to delete`);
    if (row.live) throw new Error(`Stop ${row.title} before permanently deleting it`);
    if (row.session && this.state.sessions.some((session) => session.sessionId === row.session?.sessionId)) {
      await this.closeResolvedSession(row.session, row.session.lifecycle === "hotRunning");
    }
    const before = this.state.forgetNativeChat(native.providerId, native.nativeSessionId);
    try {
      await this.runtime.deleteNative(native);
    } catch (error) {
      if (before) this.state.setNativeCatalogue(before);
      throw error;
    }
  }

  /// Archive a provider-owned conversation after one explicit confirmation.
  async archiveConversation(value: ConversationItem | Conversation): Promise<void> {
    const row = value instanceof ConversationItem ? value.conversation : value;
    const capabilities = await this.capabilitiesFor(row.providerId);
    const decision = conversationArchival(row, capabilities);
    if (decision.kind === "unsupported") {
      void vscode.window.showInformationMessage(decision.why);
      return;
    }
    const question = archivalQuestion(row);
    const choice = await vscode.window.showWarningMessage(
      question.message,
      { modal: true, detail: question.detail },
      question.button,
    );
    if (choice !== question.button) return;
    await this.archiveNativeWithoutAsking(row);
  }

  async archiveNativeWithoutAsking(row: Conversation): Promise<void> {
    const native = row.native;
    if (!native) throw new Error(`${row.title} has nothing left to archive`);
    if (row.session && this.state.sessions.some((session) => session.sessionId === row.session?.sessionId)) {
      await this.closeResolvedSession(row.session, row.session.lifecycle === "hotRunning");
    }
    await this.runtime.archiveNative(native);
    await this.loadNativeChats(row.providerId, true);
  }

  async close(value?: ConversationItem | SessionLine): Promise<void> {
    const row = value instanceof ConversationItem ? value.conversation : null;
    if (row?.presence.kind === "hosted") {
      await this.stopHosted(row);
      return;
    }
    const session = this.sessionOf(value);
    const action = session.lifecycle === "hotRunning" ? "Stop and close" : "Close in Runtrol";
    const project = this.state.conversationOf(session.sessionId)?.folder || path.basename(session.workspace);
    const choice = await vscode.window.showWarningMessage(
      `Close the ${session.providerId} chat in ${project}?`,
      {
        modal: true,
        detail: "Runtrol stops supervising this chat. The coding service keeps its own chat history.",
      },
      action,
    );
    if (choice !== action) {
      return;
    }
    await this.closeResolvedSession(session, session.lifecycle === "hotRunning");
  }

  /// Stop a conversation Runtrol hosts without supervising: the service's own terminal interface in a PTY that
  /// some Runtime generation owns. Every conversation kept alive across an update is one of these, and until
  /// now its Stop went looking for a supervised session and failed (measured 2026-08-29). The process ends in
  /// the generation that runs it, which is also what lets that generation finish draining.
  private async stopHosted(row: Conversation): Promise<void> {
    const choice = await vscode.window.showWarningMessage(
      `Stop ${row.title}?`,
      {
        modal: true,
        detail: "Runtrol ends this conversation's process. The coding service keeps its own chat history.",
      },
      "Stop",
    );
    if (choice !== "Stop") return;
    await this.stopHostedResolved(row);
  }

  /// Stop an already resolved hosted row without opening the confirmation UI. The installed-host journey has
  /// already made that decision before reaching this boundary.
  async stopHostedResolved(row: Conversation): Promise<void> {
    if (row.presence.kind !== "hosted") {
      throw new Error(`${row.title} is not a Runtime-hosted terminal`);
    }
    await this.runtime.stopTerminal(row.presence.terminal);
  }

  async closeResolvedSession(
    value: ConversationItem | SessionLine | string,
    interruptRunning: boolean,
  ): Promise<void> {
    const id = typeof value === "string"
      ? value
      : value instanceof ConversationItem
        ? value.conversation.session?.sessionId ?? ""
        : value.sessionId;
    const session = this.state.sessions.find((candidate) => candidate.sessionId === id);
    if (!session) {
      throw new Error("that session is no longer listed");
    }
    await this.runtime.close(runtimeAction(session), interruptRunning);
    try {
      const released = await this.isolatedWorkspaces.release(
        session.workspace,
        null,
        session.sessionId,
      );
      if (released?.outcome === "preservedDirty") {
        const action = await vscode.window.showWarningMessage(
          "Runtrol kept the isolated workspace because it contains changes.",
          { modal: false, detail: released.workspace },
          "Open worktree",
        );
        if (action === "Open worktree") {
          await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(released.workspace), {
            forceNewWindow: true,
          });
        }
      }
      await this.refreshIsolatedWorkspaces();
    } catch (error) {
      void vscode.window.showWarningMessage(
        `The chat closed, but its isolated workspace could not be cleaned up: `
        + `${error instanceof Error ? error.message : String(error)}`,
      );
    }
    await this.refresh();
  }

  async openWorkspace(value?: ConversationItem | SessionLine): Promise<void> {
    const session = this.sessionOf(value);
    await this.persistSelection(session.sessionId);
    if (workspaceIsOpen(session.workspace)) {
      await vscode.commands.executeCommand("workbench.view.explorer");
      return;
    }
    await this.switchWindowTo(session.workspace);
  }

  dispose(): void {
    this.disposed = true;
    this.indexAbort?.abort();
    this.cancelNativeDiscoveries();
    this.client.dispose();
    this.runtime.dispose();
  }

  selectionPersisted(): Promise<void> {
    return this.selectionPersistenceTail;
  }

  private persistSelection(sessionId: string): Promise<void> {
    const stored = this.selectionPersistenceTail.then(() => this.selection.save(sessionId));
    this.selectionPersistenceTail = stored.catch(() => undefined);
    return stored;
  }

  private clearPersistedSelection(): Promise<void> {
    const cleared = this.selectionPersistenceTail.then(() => this.selection.clear());
    this.selectionPersistenceTail = cleared.catch(() => undefined);
    return cleared;
  }

  private startSessionIndexWatch(): void {
    this.indexAbort?.abort();
    const abort = new AbortController();
    this.indexAbort = abort;
    void this.sessionIndexLoop(abort.signal);
    void this.memoryLoop(abort.signal);
    void this.nativeActivityLoop(abort.signal);
  }

  /// Ask the Runtime every few seconds what each conversation's process holds in memory.
  ///
  /// A poll rather than a watch, on purpose: the watches carry structural changes, and a memory figure moves
  /// while nothing structural does. One listing pair every five seconds costs the Runtime two local calls,
  /// and a failed round is simply the next round's problem; the watch loop is what reports reachability.
  private async memoryLoop(signal: AbortSignal): Promise<void> {
    while (!signal.aborted && !this.disposed) {
      await abortableDelay(MEMORY_POLL_MS, signal);
      // Waits out a foreground action: the poll shares the client's one serialised lane with whatever the
      // person is doing, and a memory figure is the last thing worth making a click wait behind.
      if (signal.aborted || this.state.coreReach !== "reached" || this.nativeDiscoveryPauseDepth > 0) continue;
      try {
        const [sessions, terminals] = await Promise.all([
          this.runtime.listSessionsNow(),
          this.runtime.listTerminals(),
        ]);
        const bySession = new Map<string, number>();
        for (const session of sessions.sessions) {
          if (typeof session.memoryBytes === "number") bySession.set(session.sessionId, session.memoryBytes);
        }
        const byNative = new Map<string, number>();
        for (const terminal of terminals.terminals) {
          if (typeof terminal.memoryBytes === "number" && terminal.nativeSessionId) {
            byNative.set(`${terminal.providerId}:${terminal.nativeSessionId}`, terminal.memoryBytes);
          }
        }
        this.state.setMemory(bySession, byNative);
      } catch (error) {
        // Reported nowhere on purpose: the index watch owns the reachability verdict and says so in its own
        // words; a missed memory round changes no row and the next round asks again.
        if (signal.aborted) return;
        void error;
      }
    }
  }

  private async refreshAfterReconnect(): Promise<void> {
    let lastError: unknown;
    for (let attempt = 0; attempt < 4; attempt += 1) {
      try {
        await this.refresh();
        return;
      } catch (error) {
        lastError = error;
        await this.client.reset();
        if (attempt < 3) {
          await delay(25 * (2 ** attempt));
        }
      }
    }
    throw lastError;
  }

  private async sessionIndexLoop(signal: AbortSignal): Promise<void> {
    let retryMs = 250;
    while (!signal.aborted && !this.disposed) {
      try {
        const connected = new AbortController();
        const watching = AbortSignal.any([signal, connected.signal]);
        await Promise.race([
          this.runtime.watchSessions(
            (listing) => {
              this.applyListing(
                listing.sessions,
                listing.warnings,
                this.state.providers,
              );
            },
            watching,
          ),
          this.runtime.watchProviders(
            (providers) => {
              this.applyListing(
                this.state.sessions,
                [],
                providers.providers,
              );
            },
            watching,
            (usage) => this.state.replaceUsage(usage.providers),
          ),
          this.runtime.watchTerminals(
            (terminals) => this.applyTerminalIndex(terminals),
            watching,
          ),
        ]).finally(() => connected.abort());
        retryMs = 250;
      } catch (error) {
        if (signal.aborted) {
          return;
        }
        // The watch is how this window stays in touch. Losing it is losing the Core, and the tree says so
        // rather than letting an empty list be read as an empty machine. The banner is the whole message: a
        // watch dies with a raw transport phrase ("Runtime closed during a frame"), and toasting that on top of
        // the banner put a protocol sentence in front of the person for something they can only wait out
        // (measured 2026-09-05, the daemon killed under an open window). Reachability is the banner's to say,
        // and the loop retries on its own; nothing is toasted here.
        this.state.setCoreReach("unreachable");
        this.revokeNativeActivityProofs();
      }
      await abortableDelay(retryMs, signal);
      retryMs = Math.min(retryMs * 2, 5_000);
    }
  }

  /// Observe provider-owned processes that began outside a Runtrol surface on a bounded fast clock.
  ///
  /// Providers answer from their small live-process roster, not from a conversation catalogue. Brokered processes
  /// still take the zero-poll terminal index path; this clock exists only for an already external owner that cannot
  /// publish into the daemon registry itself.
  private async nativeActivityLoop(signal: AbortSignal): Promise<void> {
    while (!signal.aborted && !this.disposed) {
      await abortableDelay(NATIVE_ACTIVITY_POLL_MS, signal);
      if (signal.aborted || this.state.coreReach !== "reached" || this.nativeDiscoveryPauseDepth > 0) continue;
      try {
        await this.pollNativeActivity(signal);
      } catch (error) {
        if (signal.aborted) return;
        this.revokeNativeActivityProofs();
        void error;
      }
    }
  }

  /// Apply one daemon-owned terminal registry snapshot immediately.
  ///
  /// A placeholder row is derived synchronously from this snapshot. Provider catalogue discovery then replaces its
  /// project fallback with the provider title without delaying the first sidebar update.
  private applyTerminalIndex(snapshot: TerminalIndexSnapshot): void {
    this.state.setTerminals(snapshot.terminals);
    this.state.setTerminalWarnings(snapshot.warnings);
    const next = new Map<string, string | null>();
    const changedProviders = new Set<string>();
    for (const terminal of snapshot.terminals) {
      if (terminal.processState !== "running") continue;
      const key = `${terminal.runtimeGeneration}:${terminal.terminalId}`;
      const native = terminal.nativeSessionId ?? null;
      next.set(key, native);
      if (!this.hostedTerminalIdentities.has(key) || this.hostedTerminalIdentities.get(key) !== native) {
        changedProviders.add(terminal.providerId);
      }
    }
    this.hostedTerminalIdentities = next;
    for (const providerId of changedProviders) this.deferNativeDiscovery(providerId, true);
    if (changedProviders.size > 0) this.scheduleNativeDiscoveries();
  }

  private applyListing(
    sessions: readonly SessionLine[],
    warnings: readonly string[],
    providers: readonly ProviderLine[],
  ): void {
    // Anything listed at all means the Core answered this window.
    const reachedBefore = this.state.coreReach === "reached";
    this.state.setCoreReach("reached");
    const titleProviders = nativeTitleRefreshProviders(this.state.sessions, sessions);
    const previousSelected = this.state.selected;
    const selected = previousSelected?.sessionId ?? null;
    this.state.replace(sessions, providers);
    const currentSelected = this.state.selected;
    void currentSelected;
    void previousSelected;
    for (const warning of warnings) {
      if (this.reportedRuntimeWarnings.remember(warning)) {
        this.say(warning, "warning");
      }
    }
    // Reaching the Core, for the first time or after losing it, is where the one-shot startup work has to
    // be picked up again: initialization and `refresh` are its only callers, and both of them throw while
    // the Core is not up yet, which is exactly when a window starts its own.
    if (!reachedBefore) {
      this.startProviderVerification(this.state.providers);
      this.startExistingChatDiscovery();
    }
    // A service that becomes usable after the last refresh has never been asked for its stored
    // conversations. The watch reports that it became usable, and reporting was all anything did.
    // Measured on the operator machine 2026-08-28: the Claude CLI was replacing itself (2.1.248 to
    // 2.1.250) while a window opened, so it was unusable at every point that asks, and its probe only
    // finished five and a half minutes later. That window then listed every project with nothing under
    // it and no notice saying why, for an hour, until Refresh Conversations was run by hand. Asked once
    // per service per window: the reconnect paths that drop the catalogues clear this with them.
    const waking = unaskedUsable(this.state.providers, this.chatDiscoveryAsked);
    for (const providerId of waking) {
      this.chatDiscoveryAsked.add(providerId);
      this.deferNativeDiscovery(providerId, false);
    }
    if (waking.length > 0) this.scheduleNativeDiscoveries();
    for (const providerId of titleProviders) this.deferNativeDiscovery(providerId, true);
    if (titleProviders.length > 0) this.scheduleNativeDiscoveries();
    if (selected && !this.state.selected) {
      void this.clearPersistedSelection().catch((error: unknown) => {
        this.say(
          `Cannot clear the selected session: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      });
    }
  }

  /// Ask each unverified coding service what it can do, one at a time.
  ///
  /// Verification starts the CLI, and the slowest installed one takes about four seconds. Firing every provider
  /// at once put that many process spawns on the machine during activation, and each completion asked for a full
  /// refresh, which asked to verify again. Measured on 2026-08-17: with four installed services that turned a
  /// refresh from immediate into more than five seconds.
  ///
  /// Serialized instead, and always in the background. One provider is probed at a time, the surface stays
  /// answerable throughout, and each service appears in the list the moment its own probe lands rather than
  /// every service appearing after the slowest one.
  private startProviderVerification(providers: readonly ProviderLine[]): void {
    for (const provider of providers) {
      if (!awaitsVerification(provider) || this.verifyingProviders.has(provider.providerId)) {
        continue;
      }
      this.verifyingProviders.add(provider.providerId);
      const verified = this.verificationTail.then(async () => {
        if (this.disposed) return;
        try {
          await this.runtime.verifyProvider(provider.providerId);
          if (!this.disposed) await this.refresh();
        } catch (error: unknown) {
          if (!this.disposed) {
            this.say(
              `Cannot verify ${provider.displayName}: ${error instanceof Error ? error.message : String(error)}`,
              "warning",
            );
          }
        } finally {
          this.verifyingProviders.delete(provider.providerId);
        }
      });
      this.verificationTail = verified;
    }
  }

  /// Ask each usable service which provider-owned processes are live and which are answering.
  ///
  /// Runtime answers both facts from the provider's bounded structural surfaces and validates recorded process
  /// identities with the operating system. Terminal output is not evidence of an open model turn: an idle TUI
  /// repaints prompts and cursors too. This dedicated 250 ms compatibility clock does not list stored
  /// conversations. Newly observed native identities trigger one targeted catalogue refresh so the placeholder
  /// can acquire the provider's title.
  private async pollNativeActivity(signal: AbortSignal): Promise<void> {
    const providers = this.state.providers.filter(isUsable);
    if (providers.length === 0) {
      this.clearNativeActivityState();
      return;
    }
    const answers = await Promise.all(providers.map(async (
      provider,
    ): Promise<readonly [string, NativeActivity | null]> => {
      // A failed read is no current process proof. The next 250 ms round can restore the row; until then the
      // old identity becomes unconfirmed so it cannot claim `Elsewhere` or race a second resume process.
      try {
        return [provider.providerId, await this.runtime.nativeActivity(provider.providerId)];
      } catch (error) {
        if (signal.aborted) throw error;
        return [provider.providerId, null];
      }
    }));
    if (signal.aborted || this.disposed) return;
    const projected = projectNativeActivity(
      answers,
      this.nativeActivityByProvider,
      this.nativeUnconfirmedByProvider,
    );
    this.applyNativeActivityProjection(projected);
    for (const providerId of projected.discoveredProviders) this.deferNativeDiscovery(providerId, true);
    this.deferUnlistedDiscoveries(projected);
    this.scheduleNativeDiscoveries();
  }

  /// Ask a provider's catalogue again while it owns a live conversation no row lists. The first ask is quick and
  /// each wait after it doubles up to a cap; the wait restarts whenever the set of unlisted conversations changes
  /// and the asking ends by itself once the row exists or the process is gone.
  private deferUnlistedDiscoveries(projected: NativeActivityProjection): void {
    const listed = new Set(
      this.state.nativeChats.map((chat) => nativeProcessKey(chat.providerId, chat.nativeSessionId)),
    );
    const unlisted = unlistedLiveProviders(projected.liveByProvider, listed);
    for (const providerId of [...this.unlistedReask.keys()]) {
      if (!unlisted.has(providerId)) this.unlistedReask.delete(providerId);
    }
    const now = Date.now();
    for (const [providerId, members] of unlisted) {
      const prior = this.unlistedReask.get(providerId);
      if (prior && prior.members === members) {
        if (now < prior.askedAt + prior.waitMs) continue;
        this.unlistedReask.set(providerId, {
          members,
          askedAt: now,
          waitMs: Math.min(prior.waitMs * 2, UNLISTED_REASK_MAX_MS),
        });
      } else {
        this.unlistedReask.set(providerId, { members, askedAt: now, waitMs: UNLISTED_REASK_FIRST_MS });
      }
      this.deferNativeDiscovery(providerId, true);
    }
  }

  private applyNativeActivityProjection(projected: NativeActivityProjection): void {
    this.nativeActivityByProvider = new Map(projected.liveByProvider);
    this.nativeAttachableByProvider = new Map(projected.attachableByProvider);
    this.nativeActiveByProvider = new Map(projected.activeByProvider);
    this.nativeUnconfirmedByProvider = new Map(projected.unconfirmedByProvider);
    this.state.setNativeActivity(projected.active);
    this.state.setObservedNative(projected.live);
    this.state.setAttachableNative(projected.attachable);
    this.state.setFocusableNative(projected.focusable);
    this.state.setUnconfirmedNative(projected.unconfirmed);
  }

  /// Revoke the live badge when Runtime proof is unavailable, while retaining a deny-only owner guard.
  private revokeNativeActivityProofs(): void {
    const providerIds = new Set([
      ...this.nativeActivityByProvider.keys(),
      ...this.nativeUnconfirmedByProvider.keys(),
    ]);
    const projected = projectNativeActivity(
      [...providerIds].map((providerId) => [providerId, null] as const),
      this.nativeActivityByProvider,
      this.nativeUnconfirmedByProvider,
    );
    this.applyNativeActivityProjection(projected);
  }

  /// Forget all observations only when there is authoritatively no usable provider to own them.
  private clearNativeActivityState(): void {
    this.nativeActivityByProvider.clear();
    this.nativeAttachableByProvider.clear();
    this.nativeActiveByProvider.clear();
    this.nativeUnconfirmedByProvider.clear();
    this.state.setNativeActivity(new Set());
    this.state.setObservedNative(new Set());
    this.state.setAttachableNative(new Set());
    this.state.setFocusableNative(new Set());
    this.state.setUnconfirmedNative(new Set());
  }

  private loadNativeChats(providerId: string, force: boolean): Promise<void> {
    const active = this.nativeDiscoveries.get(providerId);
    if (active) {
      // A forced ask means "read again after whatever is in flight", whether or not that read was forced too:
      // the read already running began before the change that forced this one (measured 2026-09-05: the read
      // that started when a terminal appeared was still running when its provider wrote the conversation down,
      // the ask made at that moment was folded into it, and the conversation stayed unlisted).
      if (!force) return active.pending;
      if (active.queuedForce) return active.queuedForce;
      const generation = this.nativeDiscoveryGeneration;
      active.queuedForce = active.pending.then(() => {
        if (this.disposed || generation !== this.nativeDiscoveryGeneration) return;
        return this.loadNativeChats(providerId, true);
      });
      return active.queuedForce;
    }
    const provider = this.state.providers.find((candidate) => candidate.providerId === providerId);
    if (!provider || !isUsable(provider)) return Promise.resolve();
    void this.loadProviderCapabilities(providerId).catch((error: unknown) => {
      if (!this.disposed) {
        this.say(
          `Cannot load conversation actions for ${provider.displayName}: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      }
    });
    if (!force && this.state.nativeCatalogue(providerId)) return Promise.resolve();
    const generation = this.nativeDiscoveryGeneration;
    const abort = new AbortController();
    const pending = this.runtime.nativeChats(providerId, abort.signal).then((catalogue) => {
      if (!this.disposed && generation === this.nativeDiscoveryGeneration && this.providerUsable(providerId)) {
        this.state.setNativeCatalogue(catalogue);
      }
    }).catch((error: unknown) => {
      if (
        !abort.signal.aborted
        && !this.disposed
        && generation === this.nativeDiscoveryGeneration
        && this.providerUsable(providerId)
      ) {
        this.state.setNativeCatalogue(nativeCatalogueAfterFailure(
          this.state.nativeCatalogue(providerId),
          providerId,
          error,
        ));
      }
    }).finally(() => {
      if (this.nativeDiscoveries.get(providerId)?.pending === pending) {
        this.nativeDiscoveries.delete(providerId);
      }
    });
    this.nativeDiscoveries.set(providerId, { abort, pending, force, queuedForce: null });
    return pending;
  }

  private capabilitiesFor(providerId: string): Promise<ProviderCapabilities | null> {
    const current = this.state.providerCapabilities(providerId);
    if (current) return Promise.resolve(current);
    return this.loadProviderCapabilities(providerId).then(() => this.state.providerCapabilities(providerId));
  }

  private loadProviderCapabilities(providerId: string): Promise<void> {
    if (this.state.providerCapabilities(providerId)) return Promise.resolve();
    const active = this.capabilityDiscoveries.get(providerId);
    if (active) return active;
    const pending = this.runtime.capabilities(providerId).then((capabilities) => {
      if (!this.disposed && this.state.providers.some((provider) => provider.providerId === providerId)) {
        this.state.setProviderCapabilities(capabilities);
      }
    }).finally(() => {
      if (this.capabilityDiscoveries.get(providerId) === pending) {
        this.capabilityDiscoveries.delete(providerId);
      }
    });
    this.capabilityDiscoveries.set(providerId, pending);
    return pending;
  }

  private cancelNativeDiscoveries(): Map<string, boolean> {
    this.nativeDiscoveryGeneration += 1;
    if (this.nativeDiscoveryRestart) {
      clearTimeout(this.nativeDiscoveryRestart);
      this.nativeDiscoveryRestart = null;
    }
    const providers = new Map(this.deferredNativeProviders);
    for (const [providerId, discovery] of this.nativeDiscoveries) {
      providers.set(
        providerId,
        (providers.get(providerId) ?? false) || discovery.force || discovery.queuedForce !== null,
      );
    }
    this.deferredNativeProviders.clear();
    for (const discovery of this.nativeDiscoveries.values()) {
      discovery.abort.abort(new Error("foreground chat action has priority"));
    }
    this.nativeDiscoveries.clear();
    return providers;
  }

  private beginForegroundAction(): Map<string, boolean> {
    this.nativeDiscoveryPauseDepth += 1;
    return this.cancelNativeDiscoveries();
  }

  private endForegroundAction(providers: ReadonlyMap<string, boolean>): void {
    for (const [providerId, force] of providers) this.deferNativeDiscovery(providerId, force);
    this.nativeDiscoveryPauseDepth = Math.max(0, this.nativeDiscoveryPauseDepth - 1);
    this.scheduleNativeDiscoveries();
  }

  private startExistingChatDiscovery(): void {
    for (const provider of this.state.providers.filter(isUsable)) {
      this.chatDiscoveryAsked.add(provider.providerId);
      this.deferNativeDiscovery(provider.providerId, false);
    }
    this.flushNativeDiscoveries();
  }

  private scheduleNativeDiscoveries(): void {
    if (
      this.disposed
      || this.nativeDiscoveryPauseDepth > 0
      || this.nativeDiscoveryRestart
      || this.deferredNativeProviders.size === 0
    ) {
      return;
    }
    this.nativeDiscoveryRestart = setTimeout(() => {
      this.nativeDiscoveryRestart = null;
      this.flushNativeDiscoveries();
    }, NATIVE_DISCOVERY_IDLE_MS);
  }

  private flushNativeDiscoveries(): void {
    if (this.nativeDiscoveryRestart) {
      clearTimeout(this.nativeDiscoveryRestart);
      this.nativeDiscoveryRestart = null;
    }
    if (this.disposed || this.nativeDiscoveryPauseDepth > 0) {
      return;
    }
    const providers = [...this.deferredNativeProviders];
    this.deferredNativeProviders.clear();
    for (const [providerId, force] of providers) this.discoverNativeChats(providerId, force);
  }

  private deferNativeDiscovery(providerId: string, force: boolean): void {
    this.deferredNativeProviders.set(
      providerId,
      (this.deferredNativeProviders.get(providerId) ?? false) || force,
    );
  }

  private providerUsable(providerId: string): boolean {
    return this.state.providers.some(
      (provider) => provider.providerId === providerId && isUsable(provider),
    );
  }

  private requireSelected(): SessionLine {
    const selected = this.state.selected;
    if (!selected) {
      throw new Error("open a conversation first");
    }
    return selected;
  }

  /// The supervised session behind a row, a record, or the open conversation.
  ///
  /// A row that no provider process is holding has no session to act on, and saying so by name is better than
  /// silently acting on whatever happened to be open instead.
  private sessionOf(value?: ConversationItem | SessionLine): SessionLine {
    if (!value) return this.requireSelected();
    if (!(value instanceof ConversationItem)) return value;
    const session = value.conversation.session;
    if (!session) {
      throw new Error(`${value.conversation.title} is not open yet`);
    }
    return session;
  }

  /// The one thing worth showing from anywhere in the window.
  ///
  /// A count of running agents is ambient information nobody acts on. A count of agents that have stopped for
  /// this person is the only number in this product that is a request, so when there is one the status bar stops
  /// reporting and starts asking, and clicking it goes straight there instead of opening a list to search.
  private updateStatus(): void {
    const waiting = attentionCount(this.state.conversations);
    const hot = this.state.sessions.filter((session) => session.hot).length;
    if (waiting > 0) {
      this.status.text = `$(bell-dot) ${waiting} waiting`;
      this.status.tooltip = waiting === 1
        ? "One conversation is waiting for you. Click to open it."
        : `${waiting} conversations are waiting for you. Click to open the next one.`;
      this.status.command = "runtrol.openNextWaiting";
      this.status.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
      return;
    }
    const selected = this.state.selected;
    const selectedProject = selected
      ? this.state.conversationOf(selected.sessionId)?.folder || path.basename(selected.workspace)
      : null;
    this.status.text = selected
      ? `$(pulse) ${selectedProject}  ${hot}/${this.state.sessions.length}`
      : `$(pulse) Runtrol  ${hot}/${this.state.sessions.length}`;
    this.status.tooltip = `${hot} running conversations, ${this.state.sessions.length} total`;
    this.status.command = "runtrol.switchSession";
    this.status.backgroundColor = undefined;
  }
}


/// Whether a started session must own its working tree alone, or may share it with another writer.
type StartDecision = "exclusive" | "shared";

type StartWorkspace = {
  workspace: string;
  access: StartDecision;
};

type StartConfiguration = {
  model: string | null;
  reasoningEffort: string | null;
  permission: string | null;
};

/// Move the remembered choice to the front of a picker and say why it leads.
///
/// Reordering, never preselecting-and-skipping: the question is still asked, the remembered answer is
/// just the one Enter lands on. Everything else keeps its order.
/// Where global state remembers the project this window was on before its last switch.
const PREVIOUS_PROJECT_KEY = "runtrol.previousProject";
/// Where global state remembers the project folders any window has listed, for the keyboard switch.
const KNOWN_PROJECTS_KEY = "runtrol.knownProjects";
const MAX_KNOWN_PROJECTS = 200;
/// Where global state remembers which conversations the operator pinned to the top. Keyed by conversation key,
/// so a pin survives a saved chat becoming a live session and back.
const PINNED_CONVERSATIONS_KEY = "runtrol.pinnedConversations";
const RENAMED_CONVERSATIONS_KEY = "runtrol.conversationNames";

/// What this window is open on, as a string `vscode.openFolder` takes back: the workspace file when there is
/// one, else the first folder; null for an empty window.
function currentWindowTarget(): string | null {
  return vscode.workspace.workspaceFile?.toString()
    ?? vscode.workspace.workspaceFolders?.[0]?.uri.toString()
    ?? null;
}

/// A QuickPick label without its `$(icon)` glyphs, for the page's popover, which draws no codicons.
function withoutCodicons(label: string): string {
  return label.replace(/\$\([a-z0-9-]+\)\s*/gu, "").trim();
}

function leadWith<T extends { description?: string }>(
  choices: T[],
  remembered: (choice: T) => boolean,
  why: string,
): T[] {
  const index = choices.findIndex(remembered);
  if (index < 0) return choices;
  const led = { ...choices[index], description: why } as T;
  return [led, ...choices.slice(0, index), ...choices.slice(index + 1)];
}

/// A row without a supervised session must carry the provider-owned chat that opens it.
///
/// Enforced here rather than assumed, because the alternative is a click that silently does nothing.
function requireNative(conversation: Conversation): NativeChatLine {
  const native = conversation.native;
  if (!native) {
    throw new Error(`${conversation.title} has nothing left to reopen`);
  }
  return native;
}

function runtimeAction(session: SessionLine) {
  return {
    sessionId: session.sessionId,
    lifecycle: session.lifecycle,
    generation: session.sessionGeneration,
    workspace: session.workspace,
  };
}

function validateSessionLabel(value: string): string | null {
  const label = value.trim();
  if ([...label].length > 80) {
    return "Use 80 characters or fewer.";
  }
  if ([...label].some((character) => forbiddenLabelCharacter(character))) {
    return "Use one visible line without bidirectional control characters.";
  }
  return null;
}

function forbiddenLabelCharacter(character: string): boolean {
  const code = character.codePointAt(0) ?? 0;
  return code < 0x20
    || (code >= 0x7f && code <= 0x9f)
    || (code >= 0x202a && code <= 0x202e)
    || (code >= 0x2066 && code <= 0x2069);
}


async function chooseWorkspace(): Promise<string | null> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.length === 1) {
    return folders[0]?.uri.fsPath ?? null;
  }
  if (folders.length > 1) {
    const selected = await vscode.window.showQuickPick(
      folders.map((folder) => ({ label: folder.name, description: folder.uri.fsPath, folder })),
      { title: "Project for the new chat" },
    );
    return selected?.folder.uri.fsPath ?? null;
  }
  const selected = await vscode.window.showOpenDialog({
    title: "Project for the new chat",
    canSelectFolders: true,
    canSelectFiles: false,
    canSelectMany: false,
  });
  return selected?.[0]?.fsPath ?? null;
}

async function chooseCollision(collisions: readonly WorkspaceCollision[]): Promise<SessionLine | null> {
  if (collisions.length === 1) {
    return collisions[0]?.session ?? null;
  }
  const selected = await vscode.window.showQuickPick(
    collisions.map(({ session }) => ({
      label: path.basename(session.workspace) || session.workspace,
      description: `${session.providerId}  ${sessionStateLabel(session)}`,
      detail: session.workspace,
      session,
    })),
    { title: "Focus a running session", placeHolder: "Select the session already modifying this workspace" },
  );
  return selected?.session ?? null;
}

function collisionDetail(collisions: readonly WorkspaceCollision[]): string {
  const visible = collisions
    .slice(0, 3)
    .map(({ session }) => `${session.providerId}: ${session.workspace}`)
    .join("\n");
  const remaining = collisions.length > 3 ? `\n${collisions.length - 3} more running sessions` : "";
  return "Starting another agent here can modify the same files. Focus an existing session, choose a separate "
    + `workspace or worktree, or explicitly continue.\n\n${visible}${remaining}`;
}

async function chooseAlternateWorkspace(
  current: string,
  sessions: readonly SessionLine[],
): Promise<string | null> {
  const candidates = new Map<string, string>();
  for (const workspace of [
    ...(vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
    ...sessions.filter((session) => !session.hot).map((session) => session.workspace),
  ]) {
    if (
      normalizePath(workspace) !== normalizePath(current)
      && workspaceCollisions(workspace, sessions).length === 0
    ) {
      candidates.set(normalizePath(workspace), workspace);
    }
  }
  const browse = { label: "$(folder-opened) Browse for another workspace or worktree", browse: true as const };
  const selected = await vscode.window.showQuickPick(
    [
      ...[...candidates.values()].map((workspace) => ({
        label: path.basename(workspace) || workspace,
        description: workspace,
        workspace,
        browse: false as const,
      })),
      browse,
    ],
    { title: "Choose a separate workspace", placeHolder: "Avoid overlapping active writers" },
  );
  if (!selected) {
    return null;
  }
  if (!selected.browse) {
    return selected.workspace;
  }
  const picked = await vscode.window.showOpenDialog({
    title: "Choose another workspace or worktree",
    canSelectFolders: true,
    canSelectFiles: false,
    canSelectMany: false,
  });
  return picked?.[0]?.fsPath ?? null;
}

/// The media type an image file's extension says, for the block the protocol carries it in.
function imageMediaType(file: string): string {
  switch (path.extname(file).toLowerCase()) {
    case ".jpg":
    case ".jpeg":
      return "image/jpeg";
    case ".gif":
      return "image/gif";
    case ".webp":
      return "image/webp";
    default:
      return "image/png";
  }
}

function workspaceIsOpen(workspace: string): boolean {
  const expected = normalizePath(workspace);
  return (vscode.workspace.workspaceFolders ?? []).some(
    (folder) => normalizePath(folder.uri.fsPath) === expected,
  );
}

function normalizePath(value: string): string {
  const normalized = path.resolve(value);
  return process.platform === "win32" ? normalized.toLocaleLowerCase("en-US") : normalized;
}

const MEMORY_POLL_MS = 5_000;
const NATIVE_ACTIVITY_POLL_MS = 250;
/// How soon a provider is asked again for a live conversation no row lists, and the most it waits between asks.
const UNLISTED_REASK_FIRST_MS = 1_000;
const UNLISTED_REASK_MAX_MS = 15_000;

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
