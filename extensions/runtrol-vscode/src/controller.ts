import { mkdir } from "node:fs/promises";
import path from "node:path";

import { PUBLIC_LIMITS, type PublicInputBlock } from "@runtrol/runtime-client";
import * as vscode from "vscode";

import type { Attachment, ConversationBinding, ConversationPanels, DraftRecord } from "./conversationPanels";
import { parallelPlacementRequirement, type StartDecision } from "./chatPlacement";
import { ConversationLauncher } from "./conversationLauncher";
import type { Place } from "./conversationSurface";
import type { MenuAnchor, MenuItem } from "./viewActions";
import type { ConversationView } from "./conversationView";
import { CoreClient } from "./core/client";
import { draftChips, newDraftId, NO_PROJECT_LABEL, type DraftState } from "./draft";
import { readGitBranch } from "./gitBranch";
import { IsolatedWorkspaces } from "./isolatedWorkspace";
import { isProjectless } from "./projectlessWorkspace";
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
  WorkspaceAccess,
} from "./runtimeTypes";
import type { Conversation } from "./conversationList";
import { attentionCount, nextNeedingYou, projects } from "./conversationList";
import { conversationDeletion } from "./conversationDeletion";
import { archivalQuestion, conversationArchival } from "./conversationArchival";
import { conversationChoices } from "./conversationPicker";
import { awaitsVerification, isUsable } from "./providerHealth";
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
import { ConversationItem } from "./trees";
import { StudioRuntimeClient } from "./runtimeClient";
import { sessionStateLabel } from "./runtimeProjection";
import type { ModelOption } from "./sessionConfiguration";
import { modelOptions, reasoningOptions, RECENT_SERVICE_KEY } from "./sessionConfiguration";
import { nativeTitleRefreshProviders } from "./nativeTitleRefresh";
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
  private readonly seenWarnings = new Set<string>();
  private readonly verifyingProviders = new Set<string>();
  private readonly nativeDiscoveries = new Map<string, NativeDiscovery>();
  private readonly capabilityDiscoveries = new Map<string, Promise<void>>();
  private readonly deferredNativeProviders = new Map<string, boolean>();
  /// The one terminal help commands are offered in, reused so repeated attempts do not stack up.
  private helpTerminal: vscode.Terminal | null = null;
  private nativeDiscoveryRestart: NodeJS.Timeout | null = null;
  private nativeDiscoveryPauseDepth = 0;
  private nativeDiscoveryGeneration = 0;
  /// One provider probe at a time. Each spawns a CLI, so the queue is what keeps activation answerable.
  private verificationTail: Promise<void> = Promise.resolve();
  private readonly isolatedWorkspaces: IsolatedWorkspaces;
  private readonly conversationLauncher: ConversationLauncher;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: CoreClient,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly panels: ConversationPanels,
    private readonly selection: SelectionStore,
    /// The projects the operator created, offered first when a draft picks its folder.
    private readonly projectRecords: { all(): readonly ProjectRecord[] },
  ) {
    this.isolatedWorkspaces = new IsolatedWorkspaces(
      client,
      () => runtime.integrationId(),
      () => runtime.reset(),
    );
    this.conversationLauncher = new ConversationLauncher(
      runtime,
      this.isolatedWorkspaces,
      () => this.refreshIsolatedWorkspaces(),
    );
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.name = "Runtrol";
    this.status.command = "runtrol.switchSession";
    this.status.show();
    context.subscriptions.push(
      this.status,
      state.onDidChange(() => this.updateStatus()),
      state.onDidChange((change) => {
        if (change === "rows") void this.rememberKnownProjects();
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
      await this.selectForInitialization(selected);
    }
    this.startSessionIndexWatch();
    this.startProviderVerification(inventory.providers.providers);
    this.startExistingChatDiscovery();
  }

  async refresh(): Promise<void> {
    const [inventory, isolated] = await Promise.all([
      this.runtime.inventory(),
      this.isolatedWorkspaces.list(),
    ]);
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
      const warning = `isolated:${workspace.workspace_id}`;
      if (this.seenWarnings.has(warning)) continue;
      this.seenWarnings.add(warning);
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
    this.state.clearNativeCatalogues();
    await this.refresh();
    this.startExistingChatDiscovery();
  }

  async refreshChats(): Promise<void> {
    await this.refresh();
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

  async checkProviderUpdates(): Promise<void> {
    const { response } = await this.client.once({ ask: "providerUpdates" });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "providerUpdates") {
      throw new Error(`Core answered provider update inspection with ${response.say}`);
    }
    const choices: Array<vscode.QuickPickItem & { update: ProviderUpdateLine }> = response.with.map((line) => {
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
    const updated = await this.client.once({
      ask: "providerUpdate",
      with: { provider: picked.update.provider },
    });
    if (updated.response.say === "failed") {
      throw new Error(updated.response.with.message);
    }
    if (updated.response.say !== "providerUpdated") {
      throw new Error(`Core answered provider update with ${updated.response.say}`);
    }
    const result = updated.response.with;
    if (result.outcome === "updated") {
      const message = `${picked.label} was updated from ${result.from} to ${result.to}.`;
      if (result.why) {
        await vscode.window.showWarningMessage(`${message} ${result.why}`);
      } else {
        await vscode.window.showInformationMessage(message);
      }
    } else if (result.outcome === "rolledBack") {
      await vscode.window.showWarningMessage(
        `${picked.label} was restored to ${result.to} after update verification failed. ${result.why ?? ""}`.trim(),
      );
    } else {
      await vscode.window.showInformationMessage(`${picked.label} is already current at ${result.to}.`);
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
      conversationChoices(rows, Date.now()),
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
    this.state.clearNativeCatalogues();
    await this.client.reset();
    await this.runtime.reset();
    await this.refreshAfterReconnect();
    // The catalogues were cleared above and the reconnect may exist precisely because the grant's roots grew
    // (a folder opened into a live window). Without restarting discovery here, stored conversations in the new
    // root stay invisible until someone happens to press refresh, which is the silent-forever failure again.
    this.startExistingChatDiscovery();
    void this.startSessionIndexWatch();
    // Every open tab re-arms against the fresh connection; each binding replays its own window.
    for (const binding of this.panels.all()) {
      binding.rewatch();
    }
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

  private selectForInitialization(value: SessionLine): Promise<void> {
    let applied = false;
    let resolveApplied: () => void = () => undefined;
    let rejectApplied: (error: unknown) => void = () => undefined;
    const ready = new Promise<void>((resolve, reject) => {
      resolveApplied = resolve;
      rejectApplied = reject;
    });
    const selected = this.selectionTail.then(() => this.selectNow(value, false, () => {
      applied = true;
      resolveApplied();
    }));
    this.selectionTail = selected.catch(() => undefined);
    void selected.catch((error: unknown) => {
      if (!applied) {
        rejectApplied(error);
        return;
      }
      this.say(
        `Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`,
        "warning",
      );
    });
    return ready;
  }

  /// The acting tab's view: the explicit session first, the focused tab as the fallback for commands
  /// that arrive without one (the palette, the tree).
  private viewOf(session: SessionLine | null): ConversationView | null {
    if (session) {
      const bound = this.panels.bindingFor(session.sessionId);
      if (bound) return bound.view;
    }
    return this.panels.focused()?.view ?? null;
  }

  /// Ambient words with no better home go to the focused tab, and nowhere when none is open, which is
  /// also what the singleton did with its panel closed.
  private say(message: string, kind: "info" | "warning" | "error" = "info", session: SessionLine | null = null): void {
    this.viewOf(session)?.status(message, kind);
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

  private async applySelection(
    value: SelectionTarget,
    reveal: boolean,
    afterApplied: () => void,
  ): Promise<void> {
    const target = this.resolve(value);
    let session: SessionLine | null;
    if ("key" in target) {
      if (!target.canOpen) throw new Error(target.blocked ?? "that conversation cannot be opened");
      session = target.session ?? await this.adoptNativeChat(requireNative(target));
      if (!session) return;
    } else {
      session = target;
    }
    if (!session.hot) {
      const access = await this.conversationSwitchDecision(session.workspace);
      if (!access) return;
      this.state.select(session.sessionId);
      const opening = await this.panels.open(session, !reveal);
      opening.view.status("Opening the saved chat...", "info");
      session = await this.resumeSession(session, access);
    }
    const stored = this.persistSelection(session.sessionId);
    // Deliberately no window-follow here. Selecting a conversation opens ITS tab beside whatever is
    // already open and NOTHING else moves (`docs/vscodeSurface.md`): moving VS Code to the project is the
    // heading's explicit button. The tab's binding owns its watch and its replay-on-rebirth, so nothing
    // here pauses or resets anybody else's conversation.
    const binding = await this.panels.open(session, !reveal);
    // Showing the tab makes its ready callback select this session. Keep this fallback after the visible
    // document is ready: selecting first asks VS Code to scroll and repaint the sidebar while it is also
    // activating and rebuilding the Webview, which makes the interaction slower without changing a pixel.
    // Preserve-focus callers do not necessarily produce a focus event, so they still need the fallback.
    this.state.select(session.sessionId);
    void stored.catch((error: unknown) => {
      binding.view.status(
        `Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`,
        "warning",
      );
    });
    afterApplied();
  }

  /// Open a conversation with the access the operator decided, on the public Runtime. A shared open is the
  /// public Runtime's one presence-gated open (a second writer in a working tree somebody is already writing
  /// in); the Runtime client confirms the person's own choice at the machine and retries, so from here both
  /// accesses are one call (measured 2026-08-21: every "Start here anyway", concurrent switch and scratch-folder
  /// path used to send shared with no confirmation, and the refusal was misread as a sign-in problem).
  private async openWithAccess(
    open:
      | { kind: "start"; providerId: string; workspace: string; model: string | null; effort: string | null; permission: string | null }
      | { kind: "adopt"; native: NativeChatLine },
    access: WorkspaceAccess,
  ): Promise<SessionLine> {
    const opened = open.kind === "start"
      ? await this.runtime.start(open.providerId, open.workspace, access, open.model, open.effort, open.permission)
      : await this.runtime.adoptNative(open.native, access);
    return opened;
  }

  private async resumeSession(session: SessionLine, access: WorkspaceAccess): Promise<SessionLine> {
    if (!session.nativeSessionId) {
      throw new Error("that cold session has no provider-owned conversation identifier to resume");
    }
    const opened = await this.runtime.resume(runtimeAction(session), access);
    const watched = this.state.sessions.find((candidate) => (
      candidate.sessionId === opened.sessionId
      && candidate.hot
      && candidate.sessionGeneration === opened.sessionGeneration
    ));
    if (watched) return watched;
    await this.refresh();
    const resumed = this.state.sessions.find((candidate) => candidate.sessionId === opened.sessionId);
    if (!resumed) {
      throw new Error("the resumed session is absent from the current session index");
    }
    return resumed;
  }

  private async adoptNativeChat(native: NativeChatLine): Promise<SessionLine | null> {
    let catalogue = this.state.nativeCatalogue(native.providerId);
    if (!catalogue || Date.now() - catalogue.loadedAtMs >= NATIVE_ADOPTION_REFRESH_MS) {
      await this.loadNativeChats(native.providerId, true);
      catalogue = this.state.nativeCatalogue(native.providerId);
    }
    const current = catalogue?.chats.find(
      (chat) => chat.nativeSessionId === native.nativeSessionId,
    );
    if (!current) {
      throw new Error(catalogue?.warning ?? "that existing chat is no longer listed by its provider");
    }
    const managed = this.state.sessions.find((session) => (
      session.sessionId === current.alreadyManagedAs
      || (session.providerId === current.providerId
        && session.nativeSessionId === current.nativeSessionId)
    ));
    if (managed) return managed;
    if (current.resume !== "available" || !current.adoptionToken) {
      throw new Error("that existing chat cannot currently be resumed by its provider");
    }
    const access = await this.conversationSwitchDecision(current.cwd);
    if (!access) return null;
    const openedId = (await this.openWithAccess({ kind: "adopt", native: current }, access)).sessionId;
    await this.refresh();
    const adopted = this.state.sessions.find((session) => session.sessionId === openedId);
    if (!adopted) {
      throw new Error("the resumed existing chat is absent from the current session index");
    }
    return adopted;
  }

  /// A click on another saved chat is a switch, not a request to add another writer.
  ///
  /// Idle provider processes are cooled without a question and their conversation pointers remain. A turn that
  /// is genuinely producing output is the only case that asks: stop it and switch, deliberately run both, or
  /// cancel. This keeps the common one-click path quiet while never interrupting active work implicitly.
  private async conversationSwitchDecision(workspace: string): Promise<WorkspaceAccess | null> {
    const collisions = workspaceCollisions(workspace, this.state.sessions);
    if (collisions.length === 0) return "exclusive";
    const working = workingCollisions(collisions);
    if (working.length === 0) {
      for (const { session } of collisions) {
        await this.runtime.cool(runtimeAction(session), false);
      }
      await this.refresh();
      return "exclusive";
    }
    const project = path.basename(workspace) || workspace;
    const action = await vscode.window.showWarningMessage(
      `${working.length === 1 ? "Another chat is" : `${working.length} chats are`} still working in ${project}.`,
      {
        modal: true,
        detail: working.length === 1
          ? "Stop its current response and switch, or keep both chats working in the same files."
          : "Stop their current responses and switch, or keep all chats working in the same files.",
      },
      "Stop and switch",
      "Keep both working",
    );
    if (action === "Keep both working") return "shared";
    if (action !== "Stop and switch") return null;
    for (const { session } of collisions) {
      await this.runtime.cool(runtimeAction(session), session.lifecycle === "hotRunning");
    }
    await this.refresh();
    return "exclusive";
  }

  /// New chat, the way the chat apps people already use begin one: a tab with the composer ready and
  /// the chips set (this window's folder, or no project when the window has none; the service used last),
  /// and nothing running until the first message. A process per click was the old shape, and a person who
  /// changes their mind had already paid for a CLI start.
  async startSession(): Promise<void> {
    await this.openDraft({ workspace: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null });
  }

  /// New chat inside one project, from its heading: the same draft tab with the folder already answered.
  async startSessionInWorkspace(workspace: string): Promise<void> {
    await this.openDraft({ workspace });
  }

  /// Open a draft tab: project, service, model, effort and mode chips, and a composer whose first message
  /// starts the conversation with exactly those choices.
  ///
  /// Defaults cost no question: the service is the one used last (or the only usable one), and the
  /// model, effort and mode are what this project's last explicit start chose, or the installed CLI's own
  /// settings. Every one of them is one click away on its chip.
  async openDraft(seed: { workspace: string | null; providerId?: string | null }): Promise<ConversationBinding> {
    const provider = seed.providerId
      ? this.state.providers.find((candidate) => candidate.providerId === seed.providerId) ?? null
      : this.preferredService();
    const remembered = seed.workspace === null ? null : this.startDefaultOf(seed.workspace);
    const recall = provider && remembered?.providerId === provider.providerId ? remembered : null;
    const draft: DraftState = {
      id: newDraftId(),
      workspace: seed.workspace,
      providerId: provider?.providerId ?? null,
      alsoProviderIds: [],
      model: recall?.model ?? null,
      effort: recall?.effort ?? null,
      permission: recall?.permission ?? null,
    };
    const binding = await this.panels.openDraft(this.draftRecord(draft, null));
    void this.refreshDraftBranch(binding, draft);
    return binding;
  }

  /// Reopen a restored draft tab with the choices it stamped.
  async restoreDraft(panel: vscode.WebviewPanel, draft: DraftState): Promise<void> {
    const binding = await this.panels.adoptDraft(panel, this.draftRecord(draft, null));
    void this.refreshDraftBranch(binding, draft);
  }

  private draftRecord(draft: DraftState, branch: string | null): DraftRecord {
    const provider = this.state.providers.find((candidate) => candidate.providerId === draft.providerId);
    return { state: draft, chips: draftChips(draft, provider?.displayName ?? null, branch) };
  }

  /// The branch is read off the folder's own repository after the tab is up, never before it.
  private async refreshDraftBranch(binding: ConversationBinding, draft: DraftState): Promise<void> {
    if (draft.workspace === null) return;
    const branch = await readGitBranch(draft.workspace);
    if (
      branch !== null
      && binding.draft?.state.id === draft.id
      && binding.draft.state.workspace === draft.workspace
    ) {
      binding.updateDraft(this.draftRecord(binding.draft.state, branch));
    }
  }

  private async amendDraft(binding: ConversationBinding, change: Partial<DraftState>): Promise<void> {
    const current = binding.draft;
    if (!current) return;
    const next: DraftState = { ...current.state, ...change };
    const branch = next.workspace === current.state.workspace ? current.chips.branch : null;
    binding.updateDraft(this.draftRecord(next, branch));
    if (next.workspace !== current.state.workspace) await this.refreshDraftBranch(binding, next);
  }

  /// Where the draft runs: no project, this window's folders, created projects, every folder the
  /// sidebar lists, or any folder on the machine.
  async pickDraftProject(binding: ConversationBinding): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const seen = new Set<string>();
    const choices: Array<vscode.QuickPickItem & { workspace: string | null; browse?: true }> = [
      {
        label: `$(comment) ${NO_PROJECT_LABEL}`,
        description: "A conversation about nothing in particular",
        workspace: null,
      },
    ];
    const add = (workspace: string, description: string): void => {
      const identity = workspaceIdentity(workspace);
      if (seen.has(identity)) return;
      seen.add(identity);
      choices.push({
        label: `$(folder) ${workspaceName(workspace) || workspace}`,
        description,
        detail: workspace,
        workspace,
      });
    };
    for (const folder of vscode.workspace.workspaceFolders ?? []) add(folder.uri.fsPath, "Open in this window");
    for (const record of this.projectRecords.all()) add(record.workspace, record.name);
    for (const row of this.state.conversations) {
      if (!row.projectless && row.homeWorkspace.trim()) add(row.homeWorkspace, row.serviceName);
    }
    choices.push({ label: "$(folder-opened) Browse for a folder...", workspace: null, browse: true });
    const picked = await this.pickFrom(
      binding,
      "project",
      "Project for this conversation",
      "Where the coding service runs",
      choices,
    );
    if (!picked) return;
    if (picked.browse) {
      const chosen = await vscode.window.showOpenDialog({
        title: "Project for this conversation",
        canSelectFolders: true,
        canSelectFiles: false,
        canSelectMany: false,
      });
      const folder = chosen?.[0]?.fsPath;
      if (!folder) return;
      await this.amendDraft(binding, { workspace: folder });
      return;
    }
    await this.amendDraft(binding, { workspace: picked.workspace });
  }

  /// Which coding service answers this draft. Every usable one is offered, the last used leading, and beneath
  /// them "Also ask <service>": the same first message goes to that service too, in its own tab (one prompt,
  /// N agents, the grid one key away). Choosing an "also" toggles it; choosing a service makes it the one
  /// this tab becomes.
  async pickDraftService(binding: ConversationBinding): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const usable = this.state.providers.filter(isUsable);
    if (usable.length === 0) throw new Error("no installed coding-agent CLI is currently usable");
    const recent = this.context.globalState.get<string>(RECENT_SERVICE_KEY);
    type Choice = {
      label: string;
      description?: string;
      icon?: string;
      current?: boolean;
      provider: ProviderLine;
      also: boolean;
    };
    const primary: Choice[] = usable.map((provider) => ({
      label: provider.displayName,
      description: provider.providerId === recent && provider.providerId !== draft.state.providerId
        ? "Last used"
        : provider.installation.version ?? "",
      icon: provider.providerId,
      current: provider.providerId === draft.state.providerId || undefined,
      provider,
      also: false,
    }));
    const others: Choice[] = usable
      .filter((provider) => provider.providerId !== draft.state.providerId)
      .map((provider) => ({
        label: `Also ask ${provider.displayName}`,
        description: draft.state.alsoProviderIds.includes(provider.providerId)
          ? "Asked too (choose again to drop)"
          : "The same first message, in its own tab",
        icon: provider.providerId,
        current: draft.state.alsoProviderIds.includes(provider.providerId) || undefined,
        provider,
        also: true,
      }));
    const picked = await this.pickFrom(
      binding,
      "service",
      "Coding service for this conversation",
      "Choose a coding service",
      [...primary, ...(draft.state.providerId ? others : [])],
    );
    if (!picked) return;
    if (picked.also) {
      const id = picked.provider.providerId;
      const current = draft.state.alsoProviderIds;
      await this.amendDraft(binding, {
        alsoProviderIds: current.includes(id) ? current.filter((other) => other !== id) : [...current, id],
      });
      return;
    }
    // A model or effort chosen for one service means nothing to another; they revert to the new service's
    // own defaults, and the chips say so. The "also" set drops the service that became primary.
    await this.amendDraft(binding, {
      providerId: picked.provider.providerId,
      alsoProviderIds: draft.state.alsoProviderIds.filter((other) => other !== picked.provider.providerId),
      model: null,
      effort: null,
      permission: null,
    });
  }

  /// The draft's model and effort, chosen in the same one menu the live chip opens: models lead
  /// (the chosen one marked), the chosen model's efforts follow under their own caption.
  async pickDraftModel(binding: ConversationBinding): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const provider = this.requireDraftProvider(draft.state);
    if (!provider) return;
    const catalogue = await this.runtime.models(provider.providerId);
    const options = modelOptions(catalogue);
    if (options.length === 0) {
      this.say(`${provider.displayName} reports no selectable models; its own settings stay in control.`, "info");
      return;
    }
    type Row = {
      label: string;
      description?: string;
      detail?: string;
      heading?: boolean;
      current?: boolean;
      act?: { kind: "model"; id: string | null } | { kind: "effort"; id: string | null };
    };
    const rows: Row[] = [
      {
        label: "Provider default",
        description: "The installed CLI's current model setting",
        current: draft.state.model === null || undefined,
        act: { kind: "model" as const, id: null },
      },
      ...options.map((option) => ({
        label: option.label,
        description: option.description,
        detail: option.detail,
        current: option.id === draft.state.model || undefined,
        act: { kind: "model" as const, id: option.id },
      })),
    ];
    const chosenModel = options.find((option) => option.id === draft.state.model)?.model ?? null;
    const efforts = reasoningOptions(catalogue, chosenModel);
    if (efforts.length > 0) {
      rows.push({ label: "Reasoning effort", heading: true });
      rows.push({
        label: "Provider default",
        description: "The installed CLI's current effort setting",
        current: draft.state.effort === null || undefined,
        act: { kind: "effort" as const, id: null },
      });
      rows.push(...efforts.map((choice) => ({
        label: choice.id,
        description: choice.description || undefined,
        current: choice.id === draft.state.effort || undefined,
        act: { kind: "effort" as const, id: choice.id },
      })));
    }
    const picked = await this.pickFrom(
      binding,
      "model",
      `${provider.displayName}: model and effort`,
      "What this conversation starts with",
      rows,
    );
    if (!picked?.act) return;
    if (picked.act.kind === "effort") {
      await this.amendDraft(binding, { effort: picked.act.id });
      return;
    }
    // The effort belongs to a model; a new model starts from that model's default.
    await this.amendDraft(binding, { model: picked.act.id, effort: null });
  }

  async pickDraftEffort(binding: ConversationBinding): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const provider = this.requireDraftProvider(draft.state);
    if (!provider) return;
    const catalogue = await this.runtime.models(provider.providerId);
    const model = modelOptions(catalogue).find((option) => option.id === draft.state.model)?.model ?? null;
    if (reasoningOptions(catalogue, model).length === 0) {
      this.say(`${provider.displayName} reports no reasoning efforts here; its own settings stay in control.`, "info");
      return;
    }
    const picked = await this.pickReasoningEffort(
      catalogue,
      model,
      `${provider.displayName}: reasoning effort`,
      draft.state.effort,
      binding,
    );
    if (picked === undefined) return;
    await this.amendDraft(binding, { effort: picked });
  }

  async pickDraftMode(binding: ConversationBinding): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const provider = this.requireDraftProvider(draft.state);
    if (!provider) return;
    const modes = provider.switchableModes ?? [];
    if (modes.length === 0) {
      this.say(`${provider.displayName} declares no switchable modes; its own surface stays in control.`, "info");
      return;
    }
    const picked = await this.pickFrom(
      binding,
      "mode",
      `${provider.displayName}: access mode`,
      "The mode this conversation starts in",
      [
        {
          label: "Provider default",
          id: null as string | null,
          description: "Use the installed CLI's current permission mode",
        },
        ...leadWith(
          modes.map((id) => ({ label: id, id: id as string | null, description: "" })),
          (choice) => choice.id === draft.state.permission,
          "Chosen",
        ),
      ],
    );
    if (!picked) return;
    await this.amendDraft(binding, { permission: picked.id });
  }

  /// Offer choices where the question was asked: in the composer, hanging from the chip that was clicked,
  /// when the conversation's page is on screen; in the command palette otherwise (a command invoked from
  /// the palette, a page not yet ready). One list, two places; the choice itself is the same object either way.
  private async pickFrom<T extends {
    label: string;
    description?: string;
    detail?: string;
    icon?: string;
    heading?: boolean;
    current?: boolean;
  }>(
    binding: ConversationBinding | undefined,
    anchor: MenuAnchor,
    title: string,
    placeHolder: string,
    choices: readonly T[],
  ): Promise<T | undefined> {
    if (binding?.view.isVisible) {
      const items: MenuItem[] = choices.map((choice, index) => ({
        id: String(index),
        label: withoutCodicons(choice.label),
        ...(choice.description ? { description: choice.description } : {}),
        ...(choice.detail ? { detail: choice.detail } : {}),
        ...(choice.icon ? { icon: choice.icon } : {}),
        ...(choice.heading ? { heading: true } : {}),
        ...(choice.current ? { current: true } : {}),
      }));
      const chosen = await binding.view.showMenu(anchor, title, items);
      if (chosen === null) return undefined;
      const picked = choices[Number(chosen)];
      // A heading is a caption, never an answer, whatever a hostile page claims was clicked.
      return picked?.heading ? undefined : picked;
    }
    // The palette path says the same groups with its own vocabulary: a separator.
    const rows = choices.map((choice) => (choice.heading
      ? { label: choice.label, kind: vscode.QuickPickItemKind.Separator }
      : choice));
    const picked = await vscode.window.showQuickPick(rows, { title, placeHolder, matchOnDescription: true, matchOnDetail: true });
    return picked && !("kind" in picked && picked.kind === vscode.QuickPickItemKind.Separator)
      ? (picked as T)
      : undefined;
  }

  private requireDraftProvider(draft: DraftState): ProviderLine | null {
    const provider = this.state.providers.find((candidate) => candidate.providerId === draft.providerId);
    if (!provider) {
      this.say("Choose a coding service first.", "info");
      return null;
    }
    return provider;
  }

  /// The first message of a draft: start the conversation with the draft's choices, make this tab its
  /// tab, and send the words. A conversation with no project runs in the scratch folder.
  async sendDraft(
    binding: ConversationBinding,
    text: string,
    parallelPlacement?: "isolated" | "shared",
  ): Promise<void> {
    const draft = binding.draft;
    if (!draft) return;
    const provider = this.requireDraftProvider(draft.state);
    if (!provider) return;
    if (!isUsable(provider)) {
      await this.reportServiceTrouble(provider, troubleOf(undefined, provider));
    }
    const workspace = draft.state.workspace ?? await this.ensureProjectlessRoot();
    const also = draft.state.alsoProviderIds
      .map((id) => this.state.providers.find((candidate) => candidate.providerId === id) ?? null)
      .filter((candidate): candidate is ProviderLine => candidate !== null && isUsable(candidate));
    const placement = parallelPlacementRequirement(
      also.length,
      isProjectless(workspace, this.state.projectlessRoot),
    );
    const decided = placement === "single"
      ? await this.startDecision(workspace, "keep")
      : placement === "sharedOnly"
        ? "shared"
        : parallelPlacement ?? await this.parallelStartDecision(workspace, also.length + 1);
    if (decided === null || decided === "another") return;
    await this.rememberService(provider.providerId);
    if (draft.state.model !== null || draft.state.effort !== null || draft.state.permission !== null) {
      await this.rememberStartDefault(workspace, {
        providerId: provider.providerId,
        model: draft.state.model,
        effort: draft.state.effort,
        permission: draft.state.permission,
      });
    }
    const pausedDiscoveries = this.beginForegroundAction();
    try {
      if (decided === "isolated") {
        binding.view.status(`Preparing separate workspaces for ${also.length + 1} services...`, "info");
      }
      const openedId = await this.conversationLauncher.openFresh(
        {
          providerId: provider.providerId,
          model: draft.state.model,
          reasoningEffort: draft.state.effort,
          permission: draft.state.permission,
        },
        workspace,
        decided,
      );
      await this.refresh();
      const session = this.state.sessions.find((candidate) => candidate.sessionId === openedId);
      if (!session) throw new Error("the started conversation is absent from the current session index");
      this.panels.becomeSession(binding, session);
      this.state.select(session.sessionId);
      void this.persistSelection(session.sessionId).catch((error: unknown) => {
        binding.view.status(
          `Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      });
      await this.send(binding, session, text);
      if (also.length > 0) await this.askOthersToo(also, workspace, decided, text, binding);
    } catch (error) {
      if (error instanceof ServiceTroubleReported) throw error;
      const trouble = troubleOf(errorKindOf(error), provider);
      if (trouble === "unknown" && errorKindOf(error) === undefined) throw error;
      await this.reportServiceTrouble(provider, trouble);
    } finally {
      this.endForegroundAction(pausedDiscoveries);
    }
  }

  /// The same first message to each further service, each in its own tab beside the first, then the grid.
  /// One service refusing (a folder it will not write in, a sign-in it needs) is said on the first tab and
  /// does not stop the others.
  private async askOthersToo(
    also: readonly ProviderLine[],
    workspace: string,
    decision: StartDecision,
    text: string,
    from: ConversationBinding,
  ): Promise<void> {
    for (const provider of also) {
      try {
        const openedId = await this.conversationLauncher.openFresh(
          {
            providerId: provider.providerId,
            model: null,
            reasoningEffort: null,
            permission: null,
          },
          workspace,
          decision,
        );
        await this.refresh();
        const session = this.state.sessions.find((candidate) => candidate.sessionId === openedId);
        if (!session) throw new Error("the started conversation is absent from the current session index");
        const binding = await this.panels.open(session, true);
        await this.runtime.submitInput(runtimeAction(session), text);
        binding.view.status("", "info");
      } catch (error) {
        from.view.status(
          `${provider.displayName} could not be asked too: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      }
    }
    await this.panels.arrangeGrid();
  }

  /// The scratch folder, created on first use. One `mkdir` of an empty directory; nothing is written in it
  /// by this extension (the coding CLI runs there, exactly as it runs in any project folder).
  private async ensureProjectlessRoot(): Promise<string> {
    const root = this.state.projectlessRoot;
    if (!root) throw new Error("this window has no folder for conversations without a project");
    await mkdir(root, { recursive: true });
    return root;
  }

  /// Add images to the next message of this tab. Read once into memory, sent once, never stored.
  async attach(binding: ConversationBinding): Promise<void> {
    const picked = await vscode.window.showOpenDialog({
      title: "Add images to the message",
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: true,
      filters: { Images: ["png", "jpg", "jpeg", "gif", "webp"] },
    });
    for (const file of picked ?? []) {
      const bytes = await vscode.workspace.fs.readFile(file);
      const base64Data = Buffer.from(bytes).toString("base64");
      if (base64Data.length > PUBLIC_LIMITS.maxAttachmentBase64Bytes) {
        this.say(`${path.basename(file.fsPath)} is larger than a message can carry.`, "warning", binding.session);
        continue;
      }
      const attachment: Attachment = {
        name: path.basename(file.fsPath),
        mediaType: imageMediaType(file.fsPath),
        base64Data,
      };
      if (!binding.addAttachment(attachment)) {
        this.say(`A message carries at most ${PUBLIC_LIMITS.maxInputImages} images.`, "warning", binding.session);
        return;
      }
    }
  }

  removeAttachment(binding: ConversationBinding, index: number): void {
    binding.removeAttachment(index);
  }

  /// Add one image pasted in the composer. The Webview has already applied the public byte bound and MIME
  /// allowlist; the binding applies the shared image-count bound and owns the bytes until the next send.
  addPastedAttachment(binding: ConversationBinding, attachment: Attachment): void {
    if (!binding.addAttachment(attachment)) {
      this.say(`A message carries at most ${PUBLIC_LIMITS.maxInputImages} images.`, "warning", binding.session);
    }
  }

  /// A live conversation's project chip: where it runs, and the one explicit way to move the window there.
  async pickProjectForLive(binding: ConversationBinding): Promise<void> {
    const session = binding.session;
    if (!session) return;
    const projectless = isProjectless(session.workspace, this.state.projectlessRoot);
    const choices: Array<vscode.QuickPickItem & { act: "window" | "draft" }> = [];
    if (!projectless && !workspaceIsOpen(session.workspace)) {
      choices.push({
        label: "$(link-external) Open this project as the window",
        detail: session.workspace,
        act: "window",
      });
    }
    choices.push({
      label: projectless ? "$(add) New conversation with no project" : "$(add) New conversation in this project",
      detail: projectless ? undefined : session.workspace,
      act: "draft",
    });
    const picked = await this.pickFrom(
      binding,
      "project",
      projectless ? NO_PROJECT_LABEL : workspaceName(session.workspace),
      projectless ? "This conversation runs with no project" : session.workspace,
      choices,
    );
    if (!picked) return;
    if (picked.act === "window") {
      await this.switchWindowTo(session.workspace);
      return;
    }
    await this.openDraft({ workspace: projectless ? null : session.workspace, providerId: session.providerId });
  }

  /// A live conversation's service chip: the service cannot change mid-conversation (it is that CLI's own
  /// session), so the chip offers the honest next thing: the same project, another service, a new tab.
  async pickServiceForLive(binding: ConversationBinding): Promise<void> {
    const session = binding.session;
    if (!session) return;
    const usable = this.state.providers.filter(isUsable);
    if (usable.length === 0) return;
    const projectless = isProjectless(session.workspace, this.state.projectlessRoot);
    const picked = await this.pickFrom(
      binding,
      "service",
      `New conversation ${projectless ? "with no project" : `in ${workspaceName(session.workspace)}`}`,
      "Choose the service for a new conversation here",
      usable.map((provider) => ({
        label: provider.displayName,
        description: provider.installation.version ?? "",
        icon: provider.providerId,
        current: provider.providerId === session.providerId || undefined,
        provider,
      })),
    );
    if (!picked) return;
    await this.openDraft({
      workspace: projectless ? null : session.workspace,
      providerId: picked.provider.providerId,
    });
  }

  /// Send the words, and any waiting images, of one tab to its session.
  private async send(binding: ConversationBinding, session: SessionLine, text: string): Promise<void> {
    const attachments = binding.takeAttachments();
    if (attachments.length === 0) {
      await this.runtime.submitInput(runtimeAction(session), text);
      return;
    }
    const blocks: PublicInputBlock[] = [
      ...(text.trim() ? [{ type: "text" as const, text }] : []),
      ...attachments.map((attachment) => ({
        type: "image" as const,
        mediaType: attachment.mediaType,
        base64Data: attachment.base64Data,
      })),
    ];
    await this.runtime.submitBlocks(runtimeAction(session), blocks);
  }

  /// Which coding service, asked only when there is a choice to make (or always, from a chip that IS the
  /// question).
  private async chooseService(always = false, binding?: ConversationBinding): Promise<ProviderLine | null> {
    const usable = this.state.providers.filter(isUsable);
    if (usable.length === 0) {
      throw new Error("no installed coding-agent CLI is currently usable");
    }
    const recent = this.context.globalState.get<string>(RECENT_SERVICE_KEY);
    const picked = usable.length === 1 && !always ? { provider: usable[0] } : await this.pickFrom(
      binding,
      "service",
      "New conversation",
      "Choose a coding service",
      usable.map((provider) => ({
        label: provider.displayName,
        description: provider.providerId === recent ? "Last used" : provider.installation.version ?? "",
        provider,
      })),
    );
    return picked?.provider ?? null;
  }

  /// The deliberate path: name the service, the model and the effort explicitly.
  ///
  /// Automatic is the default, not the ceiling. Someone who came here specifically to run a different model gets
  /// every choice the installed CLI actually reports, and nobody else is asked.
  async startConfiguredSession(): Promise<void> {
    const provider = await this.chooseService();
    if (!provider) return;
    const selectedWorkspace = await this.chooseStartWorkspace();
    if (!selectedWorkspace) return;
    await this.finishConfiguredStart(provider, selectedWorkspace.workspace, selectedWorkspace.access);
  }

  /// The deliberate path from a project heading: the folder is already answered, the rest is asked.
  async startConfiguredSessionInWorkspace(workspace: string): Promise<void> {
    const provider = await this.chooseService();
    if (!provider) return;
    const decided = await this.startDecision(workspace, "keep");
    if (decided === null || decided === "another") return;
    await this.finishConfiguredStart(provider, workspace, decided);
  }

  private async finishConfiguredStart(
    provider: ProviderLine,
    workspace: string,
    access: StartDecision,
  ): Promise<void> {
    const configuration = await this.chooseStartConfiguration(provider, workspace);
    if (!configuration) return;
    await this.rememberService(provider.providerId);
    // Remembered to pre-highlight this project's next configured start, never to skip a question.
    await this.rememberStartDefault(workspace, {
      providerId: provider.providerId,
      model: configuration.model,
      effort: configuration.reasoningEffort,
      permission: configuration.permission,
    });
    await this.startResolvedSession(
      provider.providerId,
      workspace,
      configuration.model,
      configuration.reasoningEffort,
      access,
      true,
      configuration.permission,
    );
  }

  private startDefaultOf(workspace: string): StartDefault | null {
    const defaults = readStartDefaults(this.context.globalState.get(START_DEFAULTS_KEY));
    return defaults[workspaceIdentity(workspace)] ?? null;
  }

  private async rememberStartDefault(
    workspace: string,
    choice: Omit<StartDefault, "atMs">,
  ): Promise<void> {
    const defaults = readStartDefaults(this.context.globalState.get(START_DEFAULTS_KEY));
    await this.context.globalState.update(
      START_DEFAULTS_KEY,
      rememberStartDefault(defaults, workspaceIdentity(workspace), choice, Date.now()),
    );
  }

  /// The coding service a new conversation should use when nobody said otherwise.
  ///
  /// The one used last, because that is what a person means by "again". Falling back to the only usable service,
  /// then to the first, keeps the very first run from asking a question with one possible answer.
  private preferredService(): ProviderLine | null {
    const usable = this.state.providers.filter(isUsable);
    const recent = this.context.globalState.get<string>(RECENT_SERVICE_KEY);
    return usable.find((provider) => provider.providerId === recent) ?? usable[0] ?? null;
  }

  private async rememberService(providerId: string): Promise<void> {
    await this.context.globalState.update(RECENT_SERVICE_KEY, providerId);
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
      const openedId = await this.conversationLauncher.openFresh(
        {
          providerId: provider.providerId,
          model,
          reasoningEffort,
          permission,
        },
        workspace,
        access,
      );
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

  /// Offer the workspace's files for an @-mention and insert the chosen path as plain text.
  ///
  /// The path is text and nothing more: what an @path means stays the coding service's business,
  /// exactly like a slash command's argument. The picker lives here because only the Extension Host
  /// may list files; the page only reports that an @ was typed.
  async insertFileMention(from?: SessionLine): Promise<void> {
    const session = this.state.selected;
    const base = session && workspaceIsOpen(session.workspace)
      ? session.workspace
      : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
    const files = await vscode.workspace.findFiles(
      "**/*",
      "**/{node_modules,.git,target,dist,out,.venv,__pycache__}/**",
      2_000,
    );
    if (files.length === 0) {
      this.viewOf(from ?? null)?.insertComposerText(null);
      this.say("No workspace files are available to mention.", "info", from ?? null);
      return;
    }
    const picked = await vscode.window.showQuickPick(
      files
        .map((file) => ({
          label: (base ? path.relative(base, file.fsPath) : file.fsPath).replaceAll("\\", "/"),
        }))
        .sort((left, right) => left.label.length - right.label.length || left.label.localeCompare(right.label)),
      { title: "Mention a file", placeHolder: "The chosen path is inserted as plain text" },
    );
    this.viewOf(from ?? null)?.insertComposerText(picked ? `${picked.label} ` : null);
  }

  /// Type a coding service's own command into the operator's terminal and stop there.
  ///
  /// `false` is the whole point of this method: the line is placed, not run. Runtrol fetching and executing
  /// on somebody's behalf is the one capability this product refused from the start, and an install button
  /// that installs is exactly that capability with a friendly label. So the person reads the command,
  /// standing in their own shell, and decides.
  private offerInTerminal(offer: HelpOffer): void {
    if (this.helpTerminal?.exitStatus !== undefined) {
      this.helpTerminal = null;
    }
    this.helpTerminal ??= vscode.window.createTerminal({ name: "Runtrol: coding service" });
    this.helpTerminal.show(true);
    this.helpTerminal.sendText(offer.command, false);
  }

  async prompt(text?: string, from?: SessionLine, binding?: ConversationBinding): Promise<void> {
    const session = from ?? this.requireSelected();
    const written = text ?? await vscode.window.showInputBox({
      title: `Prompt ${path.basename(session.workspace)}`,
      prompt: "The text is sent unchanged to the provider CLI.",
      ignoreFocusOut: true,
    });
    if (!written?.trim() && (binding?.attachments.length ?? 0) === 0) {
      return;
    }
    if (binding) {
      await this.send(binding, session, written ?? "");
      return;
    }
    await this.runtime.submitInput(runtimeAction(session), written ?? "");
  }

  async submitResolvedInput(sessionId: string, text: string): Promise<void> {
    await this.refresh();
    const session = this.state.sessions.find((candidate) => candidate.sessionId === sessionId);
    if (!session) {
      throw new Error("the Mission session is no longer listed by Runtime");
    }
    await this.runtime.submitInput(runtimeAction(session), text);
  }

  async interrupt(from?: SessionLine): Promise<void> {
    const session = from ?? this.requireSelected();
    await this.runtime.interrupt(runtimeAction(session));
  }

  async openConversation(): Promise<void> {
    const selected = this.state.selected;
    if (!selected) return;
    await this.panels.open(selected);
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

  /// Add one more installed service to the focused draft from the Command Palette. This is the keyboard path to
  /// the service chip's "Also ask" choice, so fan-out needs no pointer choreography and keeps one first message.
  async alsoAskFocusedDraft(): Promise<void> {
    const binding = this.panels.focused();
    const draft = binding?.draft;
    if (!binding || !draft) {
      this.say("Open a new conversation draft before adding another service.", "info");
      return;
    }
    const choices = this.state.providers.filter((provider) => (
      isUsable(provider)
      && provider.providerId !== draft.state.providerId
      && !draft.state.alsoProviderIds.includes(provider.providerId)
    ));
    if (choices.length === 0) {
      this.say("No other installed coding service is available for this draft.", "info");
      return;
    }
    const picked = await vscode.window.showQuickPick(
      choices.map((provider) => ({
        label: provider.displayName,
        description: provider.installation.version ?? undefined,
        provider,
      })),
      {
        title: "Also ask another service",
        placeHolder: "The same first message opens in a separate workspace",
      },
    );
    if (!picked) return;
    await this.amendDraft(binding, {
      alsoProviderIds: [...draft.state.alsoProviderIds, picked.provider.providerId],
    });
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
    this.offerInTerminal(signIn);
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

  /// Show a conversation in one of the window's places: a tab, the bottom panel, the secondary side bar.
  /// From a row (opening or adopting it first, exactly as a click would), or the selected conversation.
  async placeConversation(place: Place, value?: ConversationItem | SessionLine): Promise<void> {
    let session = await this.sessionToPlace(value);
    if (!session.hot) {
      const access = await this.conversationSwitchDecision(session.workspace);
      if (!access) return;
      this.state.select(session.sessionId);
      const opening = await this.panels.openIn(session, place, true);
      opening.view.status("Opening the saved chat...", "info");
      session = await this.resumeSession(session, access);
    }
    this.state.select(session.sessionId);
    await this.panels.openIn(session, place);
  }

  /// "Also ask", for the journey and the eye pass: the same amendment the service chip's menu makes.
  async alsoAskForJourney(binding: ConversationBinding, providerId: string): Promise<void> {
    const draft = binding.draft;
    if (!draft || draft.state.alsoProviderIds.includes(providerId)) return;
    await this.amendDraft(binding, { alsoProviderIds: [...draft.state.alsoProviderIds, providerId] });
  }

  /// What the back key would return to, for the harness.
  previousProjectForJourney(): string | null {
    return this.context.globalState.get<string>(PREVIOUS_PROJECT_KEY) ?? null;
  }

  /// The grid, for the journey and the eye pass: the numbers rather than the sentence.
  arrangeGridForJourney(): Promise<{ arranged: number; leftInPlace: number }> {
    return this.panels.arrangeGrid();
  }

  /// Spread the open conversation tabs over a grid of editor groups; one command, one screen of agents.
  async arrangeConversationGrid(): Promise<void> {
    const result = await this.panels.arrangeGrid();
    const sentence = result.arranged === 0
      ? "Open a conversation or two first; the grid arranges the conversation tabs that are open."
      : result.leftInPlace === 0
        ? `${result.arranged} conversations arranged in a grid.`
        : `${result.arranged} conversations arranged in a grid; ${result.leftInPlace} left where they were (nine is the most the editor addresses by column).`;
    vscode.window.setStatusBarMessage(sentence, 4_000);
  }

  private async sessionToPlace(value?: ConversationItem | SessionLine): Promise<SessionLine> {
    if (!value) return this.requireSelected();
    if (!(value instanceof ConversationItem)) return value;
    const row = value.conversation;
    if (row.session) return row.session;
    if (!row.canOpen) throw new Error(row.blocked ?? "that conversation cannot be opened");
    const adopted = await this.adoptNativeChat(requireNative(row));
    if (!adopted) throw new Error(`${row.title} could not be opened`);
    return adopted;
  }

  async revealConversationOnEntry(): Promise<void> {
    const selected = this.state.selected;
    if (!selected || this.panels.focused()) return;
    await this.panels.open(selected, true);
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
    if (decision.kind === "forgetSupervised") {
      if (!row.session) return;
      // A pointer to a conversation the service no longer lists names nothing: forgetting it is the whole
      // deletion, and a question about it would be a question about nothing. Only an agent working right
      // now is worth a word first, because forgetting stops it mid-turn.
      if (row.session.lifecycle === "hotRunning") {
        await this.close(row.session);
        return;
      }
      await this.closeResolvedSession(row.session, false);
      void vscode.window.showInformationMessage(`Deleted ${row.title} from Runtrol.`);
      return;
    }
    const title = row.title;
    const serviceName = row.serviceName;
    await this.deleteNativeWithoutAsking(row);
    // The row simply disappears otherwise, which is the same thing a misclick looks like. Naming what left and
    // whose list it left says which of the two just happened. It claims nothing about getting it back, because
    // that is each service's own business and not the same answer for all of them.
    void vscode.window.showInformationMessage(`Deleted ${title} from ${serviceName}.`);
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
    this.panels.bindingFor(session.sessionId)?.dispose();
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

  selectedWatchReady(): Promise<void> {
    const selected = this.state.selected;
    const binding = selected ? this.panels.bindingFor(selected.sessionId) : this.panels.focused();
    return binding?.settled() ?? Promise.resolve();
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
        ]).finally(() => connected.abort());
        retryMs = 250;
      } catch (error) {
        if (signal.aborted) {
          return;
        }
        this.say(error instanceof Error ? error.message : String(error), "error");
      }
      await abortableDelay(retryMs, signal);
      retryMs = Math.min(retryMs * 2, 5_000);
    }
  }

  private applyListing(
    sessions: readonly SessionLine[],
    warnings: readonly string[],
    providers: readonly ProviderLine[],
  ): void {
    const titleProviders = nativeTitleRefreshProviders(this.state.sessions, sessions);
    const previousSelected = this.state.selected;
    const selected = previousSelected?.sessionId ?? null;
    this.state.replace(sessions, providers);
    const currentSelected = this.state.selected;
    void currentSelected;
    void previousSelected;
    // Every open tab receives its own row's fresh metadata; a row that vanished closes its tab, because
    // a tab for a session the daemon no longer lists is a view of nothing.
    for (const binding of this.panels.all()) {
      const shown = binding.session;
      // A draft has no session to vanish; it lives until it starts or its tab closes.
      if (!shown) continue;
      const row = sessions.find((candidate) => candidate.sessionId === shown.sessionId);
      if (row) {
        binding.updateSession(row);
      } else {
        binding.dispose();
      }
    }
    for (const warning of warnings) {
      if (!this.seenWarnings.has(warning)) {
        this.seenWarnings.add(warning);
        this.say(warning, "warning");
      }
    }
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

  private loadNativeChats(providerId: string, force: boolean): Promise<void> {
    const active = this.nativeDiscoveries.get(providerId);
    if (active) {
      if (!force || active.force) return active.pending;
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
        this.panels.refreshTitles(providerId);
      }
    }).catch((error: unknown) => {
      if (
        !abort.signal.aborted
        && !this.disposed
        && generation === this.nativeDiscoveryGeneration
        && this.providerUsable(providerId)
      ) {
        this.state.setNativeCatalogue(nativeCatalogueFailure(providerId, error));
        this.panels.refreshTitles(providerId);
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

  /// Switch the answering model of the open conversation, from the conversation's own header.
  ///
  /// The choices are the session's own announced set when the provider gave one (some announce it per
  /// session), and the installed CLI's reported catalogue otherwise. Runtrol relays the pick through the
  /// provider's own switch surface and displays only what the provider says back; a service with neither an
  /// announced set nor a catalogue keeps model choice in its own settings, and this says so instead of
  /// inventing a picker with nothing true to offer.
  /// One menu for the model and its reasoning effort, hanging from the one chip that shows both
  /// (the operator named the ChatGPT and Claude composers as the reference). The models lead, the
  /// current one marked; the current model's efforts follow under their own caption. Choosing a
  /// model keeps the current effort when the new model reports it and returns to that model's own
  /// default otherwise; choosing an effort keeps the model. Never a second popover after the first.
  async switchModel(
    available: readonly string[],
    currentModel: string,
    currentEffort: string,
    binding?: ConversationBinding,
  ): Promise<void> {
    if (binding?.draft) {
      await this.pickDraftModel(binding);
      return;
    }
    const session = binding?.session ?? this.state.selected;
    if (!session) return;
    const catalogue = await this.runtime.models(session.providerId);
    const options = modelOptions(catalogue);
    type Row = {
      label: string;
      description?: string;
      detail?: string;
      heading?: boolean;
      current?: boolean;
      act?: { kind: "model"; id: string; model: ModelOption["model"] } | { kind: "effort"; id: string };
    };
    // The installed CLI's catalogue when it reports one (names, descriptions, per-model efforts);
    // the session's own announced switchable set otherwise. Both are the provider's own words.
    const rows: Row[] = options.length > 0
      ? options.map((option) => ({
        label: option.label,
        description: option.description,
        detail: option.detail,
        current: option.id === currentModel || undefined,
        act: { kind: "model" as const, id: option.id, model: option.model },
      }))
      : available.map((id) => ({
        label: id,
        current: id === currentModel || undefined,
        act: { kind: "model" as const, id, model: null },
      }));
    if (rows.length === 0) {
      this.say(
        `${providerDisplayName(session.providerId, this.state.providers)} reports no switchable models; its own settings stay in control.`,
        "info",
      );
      return;
    }
    // Efforts ride the same menu, offered only when they can be true: the answering model is known
    // and the provider has not refused mid-conversation effort switches.
    const effortCapability = (await this.runtime.capabilities(session.providerId)).setReasoningEffort;
    const currentOption = options.find((option) => option.id === currentModel) ?? null;
    const efforts = currentModel && (!effortCapability || effortCapability.availability === "available")
      ? reasoningOptions(catalogue, currentOption?.model ?? null)
      : [];
    if (efforts.length > 0) {
      rows.push({ label: "Reasoning effort", heading: true });
      rows.push(...efforts.map((choice) => ({
        label: choice.id,
        description: choice.description || undefined,
        current: choice.id === currentEffort || undefined,
        act: { kind: "effort" as const, id: choice.id },
      })));
    }
    const picked = await this.pickFrom(
      binding,
      "model",
      "Model and reasoning effort",
      "What answers this conversation",
      rows,
    );
    if (!picked?.act) return;
    if (picked.act.kind === "effort") {
      await this.runtime.setModel(runtimeAction(session), currentModel, picked.act.id);
      this.viewOf(this.state.selected)?.switchRequested("effort", picked.act.id);
      return;
    }
    const kept = reasoningOptions(catalogue, picked.act.model).some((choice) => choice.id === currentEffort)
      ? currentEffort
      : undefined;
    await this.switchSelectedModel(picked.act.id, kept);
  }

  /// The relay itself, shared by the picker above and the journey proof: one path to the provider's own
  /// switch surface, so the two can never drift.
  async switchSelectedModel(model: string, reasoningEffort?: string): Promise<void> {
    const session = this.state.selected;
    if (!session) {
      throw new Error("no conversation is selected");
    }
    await this.runtime.setModel(runtimeAction(session), model, reasoningEffort);
    // Sent, not confirmed: the chip shows the request as a suffix until the provider's own
    // announcement replaces it (or the turn ends without one).
    this.viewOf(this.state.selected)?.switchRequested("model", model);
    if (reasoningEffort !== undefined) {
      this.viewOf(this.state.selected)?.switchRequested("effort", reasoningEffort);
    }
  }

  /// Switch only the reasoning effort, keeping the model the provider says is answering.
  ///
  /// `sessions/setModel` is the one switch surface and it requires the model, so the page reports
  /// which one is currently answering. A conversation whose provider never announced an answering
  /// model has nothing true to attach an effort to, and this says so instead of guessing one.
  async switchEffort(currentModel: string, binding?: ConversationBinding): Promise<void> {
    if (binding?.draft) {
      await this.pickDraftEffort(binding);
      return;
    }
    const session = binding?.session ?? this.state.selected;
    if (!session) return;
    // The provider's own word on whether a mid-session effort switch exists, read before the
    // attempt so a service that cannot do it says "from the next conversation" instead of failing.
    const capability = (await this.runtime.capabilities(session.providerId)).setReasoningEffort;
    if (capability && capability.availability !== "available") {
      const name = providerDisplayName(session.providerId, this.state.providers);
      this.say(
        `${name} cannot switch the reasoning effort mid-conversation`
        + `${capability.why ? ` (${capability.why})` : ""}. `
        + 'Start one with "New Conversation with Service, Model and Effort..." instead.',
        "info",
      );
      return;
    }
    const catalogue = await this.runtime.models(session.providerId);
    let selectedModel = currentModel;
    let model = modelOptions(catalogue).find((option) => option.id === currentModel)?.model ?? null;
    if (!selectedModel) {
      const pickedModel = await this.pickFrom(
        binding,
        "effort",
        "Choose the model for this effort",
        "This conversation has not announced its current model",
        modelOptions(catalogue),
      );
      if (!pickedModel) return;
      selectedModel = pickedModel.id;
      model = pickedModel.model;
    }
    const efforts = reasoningOptions(catalogue, model);
    if (efforts.length === 0) {
      this.say(
        `${providerDisplayName(session.providerId, this.state.providers)} reports no reasoning efforts for ${selectedModel}; its own settings stay in control.`,
        "info",
      );
      return;
    }
    const picked = await this.pickReasoningEffort(catalogue, model, "Switch reasoning effort", null, binding);
    if (picked === undefined) return;
    await this.runtime.setModel(runtimeAction(session), selectedModel, picked ?? undefined);
    this.viewOf(this.state.selected)?.switchRequested("effort", picked ?? "default");
  }

  /// Switch the governing permission mode of the open conversation, from its own header chip.
  ///
  /// The choices are the session's own announced set when the protocol gave one, and the service's
  /// manifest-declared switchable set otherwise (the same boundary the daemon enforces, so nothing offered
  /// here can be refused there as out of vocabulary). A service with neither keeps mode in its own surface,
  /// and this says so instead of inventing a picker with nothing true to offer.
  async switchMode(available: readonly string[], binding?: ConversationBinding): Promise<void> {
    if (binding?.draft) {
      await this.pickDraftMode(binding);
      return;
    }
    const session = binding?.session ?? this.state.selected;
    if (!session) return;
    const provider = this.state.providers.find(
      (candidate) => candidate.providerId === session.providerId,
    );
    const choices = available.length > 0 ? available : (provider?.switchableModes ?? []);
    if (choices.length === 0) {
      this.say(
        `${providerDisplayName(session.providerId, this.state.providers)} announces no switchable modes; its own surface stays in control.`,
        "info",
      );
      return;
    }
    const picked = await this.pickFrom(
      binding,
      "mode",
      "Switch mode",
      "Modes this service accepts a switch to",
      choices.map((id) => ({ label: id })),
    );
    if (!picked) return;
    await this.switchSelectedMode(picked.label);
  }

  /// The relay itself, shared by the picker above and the journey proof: one path to the provider's own
  /// switch surface, so the two can never drift.
  async switchSelectedMode(mode: string): Promise<void> {
    const session = this.state.selected;
    if (!session) {
      throw new Error("no conversation is selected");
    }
    await this.runtime.setMode(runtimeAction(session), mode);
    this.viewOf(this.state.selected)?.switchRequested("mode", mode);
  }

  /// One effort picker for every path that asks: `undefined` is a cancel, `null` is the provider's default.
  private async pickReasoningEffort(
    catalogue: ModelCatalog,
    model: ModelOption["model"],
    title: string,
    preferred: string | null = null,
    binding?: ConversationBinding,
  ): Promise<string | null | undefined> {
    const efforts = reasoningOptions(catalogue, model);
    if (efforts.length === 0) return null;
    const effortChoices = leadWith(
      efforts.map((choice) => ({
        label: choice.id,
        id: choice.id as string | null,
        description: choice.description || "Reported by the installed CLI",
      })),
      (choice) => choice.id === preferred && preferred !== null,
      "Last used here",
    );
    const picked = await this.pickFrom(
      binding,
      "effort",
      title,
      "Choose an effort reported for this model",
      [
        {
          label: "Provider default",
          id: null as string | null,
          description: "Use the installed CLI's current effort setting",
        },
        ...effortChoices,
      ],
    );
    return picked ? picked.id : undefined;
  }

  private async chooseStartConfiguration(
    provider: ProviderLine,
    workspace: string,
  ): Promise<StartConfiguration | null> {
    const catalogue = await this.runtime.models(provider.providerId);
    // What this project's last configured start chose, used only to lead each list: the question is
    // still asked, so the installed CLI's own settings stay the only automatic authority.
    const remembered = this.startDefaultOf(workspace);
    const recall = remembered?.providerId === provider.providerId ? remembered : null;
    const choices = leadWith(
      modelOptions(catalogue),
      (choice) => choice.id === recall?.model,
      "Last used here",
    );
    // A question with one possible answer is not a choice. When the installed CLI reports no selectable
    // catalogue, the only available answer is whatever that CLI already uses, and a picker saying so costs a
    // keystroke to convey nothing. Effort is still asked about below when this CLI reports any, because that
    // can be offered without a model catalogue.
    const selectedModel:
      | { readonly id: string | null; readonly model: ModelOption["model"] }
      | undefined =
      choices.length === 0
        ? { id: null, model: null }
        : await vscode.window.showQuickPick(
          [
            {
              label: "Provider default",
              id: null,
              model: null,
              description: "Use the installed CLI's current model setting",
            },
            ...choices,
          ],
          {
            title: `New chat with ${provider.displayName}: model`,
            placeHolder: "Choose a model reported by the installed CLI",
          },
        );
    if (!selectedModel) return null;

    const selectedEffort = await this.pickReasoningEffort(
      catalogue,
      selectedModel.model,
      `New chat with ${provider.displayName}: reasoning effort`,
      recall?.effort ?? null,
    );
    if (selectedEffort === undefined) return null;

    // The starting permission mode, offered from the same switchable vocabulary the daemon enforces
    // (so nothing offered here can be refused there), and only when this service declares one. The
    // modes that remove safety prompts are absent from that vocabulary by construction.
    const modes = provider.switchableModes ?? [];
    let permission: string | null = null;
    if (modes.length > 0) {
      const modeChoices = leadWith(
        modes.map((id) => ({ label: id, id: id as string | null, description: "" })),
        (choice) => choice.id === recall?.permission,
        "Last used here",
      );
      const pickedMode = await vscode.window.showQuickPick(
        [
          {
            label: "Provider default",
            id: null as string | null,
            description: "Use the installed CLI's current permission mode",
          },
          ...modeChoices,
        ],
        {
          title: `New chat with ${provider.displayName}: permission mode`,
          placeHolder: "Choose the mode this conversation starts in",
        },
      );
      if (!pickedMode) return null;
      permission = pickedMode.id;
    }
    return { model: selectedModel.id, reasoningEffort: selectedEffort, permission };
  }

  private async chooseStartWorkspace(): Promise<StartWorkspace | null> {
    let workspace = await chooseWorkspace();
    while (workspace) {
      const decided = await this.startDecision(workspace, "offer");
      if (decided === "another") {
        workspace = await chooseAlternateWorkspace(workspace, this.state.sessions);
        continue;
      }
      return decided === null ? null : { workspace, access: decided };
    }
    return null;
  }

  /// How a start in this folder proceeds when other chats are already writing there.
  ///
  /// One dialog for every start path, so the writer-collision vocabulary cannot drift between them. The paths
  /// differ in exactly one way: a start whose folder was chosen in the dialog may choose another, and a start
  /// from a project heading has already said which folder, so offering another would contradict the click.
  private async startDecision(
    workspace: string,
    alternatives: "offer" | "keep",
  ): Promise<StartDecision | "another" | null> {
    const collisions = workspaceCollisions(workspace, this.state.sessions);
    if (collisions.length === 0) return "exclusive";
    // Two agents answering unrelated questions in the scratch folder are not two agents editing one
    // repository; the writer question is noise there, and a conversation with no project never asks it.
    if (isProjectless(workspace, this.state.projectlessRoot)) return "shared";
    const buttons = alternatives === "offer"
      ? ["Start isolated", "Focus existing", "Choose another", "Start here anyway"]
      : ["Start isolated", "Focus existing", "Start here anyway"];
    const action = await vscode.window.showWarningMessage(
      `${path.basename(workspace)} overlaps ${collisions.length} running chat${
        collisions.length === 1 ? "" : "s"
      }.`,
      {
        modal: true,
        detail: `${collisionDetail(collisions)}\n\nStart isolated creates a clean linked checkout automatically.`,
      },
      ...buttons,
    );
    if (action === "Start isolated") return "isolated";
    if (action === "Start here anyway") return "shared";
    if (action === "Focus existing") {
      const existing = await chooseCollision(collisions);
      if (existing) {
        await this.select(existing);
      }
      return null;
    }
    return action === "Choose another" ? "another" : null;
  }

  /// A parallel team introduces its own writer collision even when no earlier chat is open. Runtrol describes
  /// both consequences and executes only the placement the person explicitly chooses.
  private async parallelStartDecision(
    workspace: string,
    services: number,
  ): Promise<"isolated" | "shared" | null> {
    const action = await vscode.window.showWarningMessage(
      `Choose where ${services} coding services work in ${path.basename(workspace)}.`,
      {
        modal: true,
        detail: "Separate worktrees creates and owns one clean linked checkout per service. "
          + "Share current checkout lets every service write the selected folder, where their edits can collide. "
          + "Runtrol does not choose either placement for you.",
      },
      "Separate worktrees",
      "Share current checkout",
    );
    if (action === "Separate worktrees") return "isolated";
    if (action === "Share current checkout") return "shared";
    return null;
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

function nativeCatalogueFailure(providerId: string, error: unknown): NativeChatCatalogue {
  return {
    providerId,
    coverage: null,
    chats: [],
    loadedAtMs: Date.now(),
    warning: `Existing chat discovery failed: ${error instanceof Error ? error.message : String(error)}`,
  };
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

function abortableDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    const timer = setTimeout(done, milliseconds);
    signal.addEventListener("abort", done, { once: true });
    function done(): void {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
  });
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
