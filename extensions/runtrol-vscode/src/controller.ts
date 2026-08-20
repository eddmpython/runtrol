import path from "node:path";

import * as vscode from "vscode";

import { ConversationView } from "./conversationView";
import { CoreClient } from "./core/client";
import type {
  ProviderUpdateLine,
  Response,
} from "./protocol";
import type {
  NativeChatCatalogue,
  NativeChatLine,
  ProviderLine,
  ModelCatalog,
  SessionLine,
  WorkspaceAccess,
} from "./runtimeTypes";
import type { Conversation } from "./conversationList";
import { attentionCount, nextNeedingYou } from "./conversationList";
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
import { providerDisplayName, sessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { ConversationItem } from "./trees";
import { StudioRuntimeClient } from "./runtimeClient";
import { sessionStateLabel } from "./runtimeProjection";
import type { ModelOption } from "./sessionConfiguration";
import { modelOptions, reasoningOptions, RECENT_SERVICE_KEY } from "./sessionConfiguration";
import {
  readStartDefaults,
  rememberStartDefault,
  START_DEFAULTS_KEY,
  type StartDefault,
} from "./startDefaults";
import { workspaceCollisions, workspaceIdentity, type WorkspaceCollision } from "./workspaceCollision";

const NATIVE_ADOPTION_REFRESH_MS = 4 * 60_000;
/// Existing conversations are discovered as soon as the surface is idle enough to ask.
///
/// Short on purpose. This delay only exists to let a foreground action finish first, never to stagger discovery
/// into a wait a person can watch.
const NATIVE_DISCOVERY_IDLE_MS = 150;

type NativeDiscovery = {
  abort: AbortController;
  pending: Promise<void>;
};

/// Every way a caller can name the conversation it wants opened.
///
/// A tree row, a row of the list itself, a session record, or a bare session identifier. They all reduce to one
/// record before anything is opened, so there is exactly one path through selection.
export type SelectionTarget = ConversationItem | Conversation | SessionLine | string;

export class Controller implements vscode.Disposable {
  private watchAbort: AbortController | null = null;
  private indexAbort: AbortController | null = null;
  private readonly status: vscode.StatusBarItem;
  private selectionTail: Promise<void> = Promise.resolve();
  private selectionPersistenceTail: Promise<void> = Promise.resolve();
  private watchReady: Promise<void> = Promise.resolve();
  private conversationVisible = false;
  private disposed = false;
  private readonly seenWarnings = new Set<string>();
  private readonly verifyingProviders = new Set<string>();
  private readonly nativeDiscoveries = new Map<string, NativeDiscovery>();
  private readonly deferredNativeProviders = new Set<string>();
  /// The one terminal help commands are offered in, reused so repeated attempts do not stack up.
  private helpTerminal: vscode.Terminal | null = null;
  private nativeDiscoveryRestart: NodeJS.Timeout | null = null;
  private nativeDiscoveryPauseDepth = 0;
  private nativeDiscoveryGeneration = 0;
  /// One provider probe at a time. Each spawns a CLI, so the queue is what keeps activation answerable.
  private verificationTail: Promise<void> = Promise.resolve();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: CoreClient,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly conversation: ConversationView,
    private readonly selection: SelectionStore,
  ) {
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.name = "Runtrol chats";
    this.status.command = "runtrol.switchSession";
    this.status.show();
    context.subscriptions.push(this.status, state.onDidChange(() => this.updateStatus()));
  }

  async initialize(): Promise<void> {
    const [inventory, remembered] = await Promise.all([
      this.runtime.inventory(),
      this.selection.load(),
    ]);
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
    } else {
      this.conversation.reset(null);
    }
    this.startSessionIndexWatch();
    this.startProviderVerification(inventory.providers.providers);
    this.startExistingChatDiscovery();
  }

  async refresh(): Promise<void> {
    const inventory = await this.runtime.inventory();
    this.applyListing(
      inventory.sessions.sessions,
      inventory.sessions.warnings,
      inventory.providers.providers,
    );
    this.startProviderVerification(inventory.providers.providers);
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
    await Promise.all(this.state.providers
      .filter(isUsable)
      .map((provider) => this.loadNativeChats(provider.providerId, true)));
  }

  discoverNativeChats(providerId: string): void {
    if (this.nativeDiscoveryPauseDepth > 0 || this.nativeDiscoveryRestart) {
      this.deferredNativeProviders.add(providerId);
      return;
    }
    void this.loadNativeChats(providerId, false);
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
        vscode.window.setStatusBarMessage("Runtrol: nothing is waiting for you.", 3_000),
      );
      return;
    }
    await this.selectConversation(next);
    await this.conversation.show();
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
    this.pauseWatch();
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
    const selected = this.state.selected;
    if (selected) {
      // Same rule as selection: the reset empties the document, so the watch replays the window.
      this.state.forgetCursor(selected.sessionId);
      this.conversation.reset(selected);
      this.ensureSelectedWatch();
    } else {
      this.conversation.reset(null);
    }
  }

  select(
    value: SelectionTarget,
    follow = true,
    reveal = true,
  ): Promise<void> {
    const selected = this.selectionTail.then(() => this.selectNow(value, follow, reveal));
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
    const selected = this.selectionTail.then(() => this.selectNow(value, false, false, () => {
      applied = true;
      resolveApplied();
    }));
    this.selectionTail = selected.catch(() => undefined);
    void selected.catch((error: unknown) => {
      if (!applied) {
        rejectApplied(error);
        return;
      }
      this.conversation.status(
        `Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`,
        "warning",
      );
    });
    return ready;
  }

  private async selectNow(
    value: SelectionTarget,
    follow: boolean,
    reveal: boolean,
    afterApplied: () => void = () => undefined,
  ): Promise<void> {
    const pausedDiscoveries = this.beginForegroundAction();
    try {
      await this.applySelection(value, follow, reveal, afterApplied);
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
    follow: boolean,
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
    if (reveal) {
      void this.conversation.show();
    }
    if (!session.hot) {
      this.pauseWatch();
      this.state.select(session.sessionId);
      this.conversation.reset(session);
      this.conversation.status("Opening the saved chat...", "info");
      session = await this.resumeSession(session);
    }
    const stored = this.persistSelection(session.sessionId);
    this.pauseWatch();
    this.state.select(session.sessionId);
    // The reset below clears the document, so the watch must replay the recent window rather than
    // resume past everything the reader can no longer see.
    this.state.forgetCursor(session.sessionId);
    this.conversation.reset(session);

    const follows = vscode.workspace.getConfiguration("runtrol").get<boolean>("followWorkspace", true);
    if (follow && follows && !workspaceIsOpen(session.workspace)) {
      await stored;
      await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(session.workspace), {
        forceNewWindow: false,
      });
      return;
    }
    void stored.catch((error: unknown) => {
      this.conversation.status(
        `Cannot remember the selected session: ${error instanceof Error ? error.message : String(error)}`,
        "warning",
      );
    });
    this.ensureSelectedWatch();
    afterApplied();
  }

  private async resumeSession(session: SessionLine): Promise<SessionLine> {
    if (!session.nativeSessionId) {
      throw new Error("that cold session has no provider-owned conversation identifier to resume");
    }
    const opened = await this.runtime.resume(runtimeAction(session), "exclusive");
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
    let access: WorkspaceAccess = "exclusive";
    const collisions = workspaceCollisions(current.cwd, this.state.sessions);
    if (collisions.length > 0) {
      const action = await vscode.window.showWarningMessage(
        `${path.basename(current.cwd)} overlaps ${collisions.length} running chat${
          collisions.length === 1 ? "" : "s"
        }.`,
        {
          modal: true,
          detail: collisionDetail(collisions),
        },
        "Focus existing",
        "Resume anyway",
      );
      if (action === "Focus existing") {
        return chooseCollision(collisions);
      }
      if (action !== "Resume anyway") return null;
      access = "shared";
    }
    const opened = await this.runtime.adoptNative(current, access);
    await this.refresh();
    const adopted = this.state.sessions.find((session) => session.sessionId === opened.sessionId);
    if (!adopted) {
      throw new Error("the resumed existing chat is absent from the current session index");
    }
    return adopted;
  }

  /// Open a new conversation without asking anything that can be answered from what is already known.
  ///
  /// A person clicking New chat has already said everything they meant to say. The coding service is the one they
  /// used last, the project is the one this window is open on, and the model and effort are whatever the installed
  /// CLI already defaults to. Every one of those was a question in an earlier design, and every one of them had a
  /// correct answer the surface could have worked out for itself.
  ///
  /// Only a genuine ambiguity still stops to ask: no project is open at all, or another agent is already writing
  /// in the same directory.
  async startSession(providerId?: string): Promise<void> {
    const provider = providerId
      ? this.state.providers.find((candidate) => candidate.providerId === providerId) ?? null
      : this.preferredService();
    if (!provider) {
      throw new Error(providerId
        ? `the installed coding service ${providerId} is no longer listed`
        : "no installed coding-agent CLI is currently usable");
    }
    if (!isUsable(provider)) {
      throw new Error(`the installed coding service ${provider.providerId} is not usable`);
    }
    const selectedWorkspace = await this.chooseStartWorkspace();
    if (!selectedWorkspace) {
      return;
    }
    await this.rememberService(provider.providerId);
    await this.startResolvedSession(
      provider.providerId,
      selectedWorkspace.workspace,
      // The installed CLI already holds the operator's own model and effort settings. Asking again would make
      // Runtrol the third place that opinion lives, and the two that disagree would both look authoritative.
      null,
      null,
      selectedWorkspace.access,
      true,
    );
  }

  /// Start a conversation inside one project, from its heading.
  ///
  /// The heading already says which folder, so that question is never asked. What remains is which coding
  /// service, and with exactly one usable service there is no question at all: the click is the whole gesture.
  /// Model and effort stay with the installed CLI's own settings, same as the quick path.
  async startSessionInWorkspace(workspace: string): Promise<void> {
    const provider = await this.chooseService();
    if (!provider) return;
    const decided = await this.startDecision(workspace, "keep");
    if (decided === null || decided === "another") return;
    await this.rememberService(provider.providerId);
    await this.startResolvedSession(provider.providerId, workspace, null, null, decided, true);
  }

  /// Which coding service, asked only when there is a choice to make.
  private async chooseService(): Promise<ProviderLine | null> {
    const usable = this.state.providers.filter(isUsable);
    if (usable.length === 0) {
      throw new Error("no installed coding-agent CLI is currently usable");
    }
    const recent = this.context.globalState.get<string>(RECENT_SERVICE_KEY);
    const picked = usable.length === 1 ? { provider: usable[0] } : await vscode.window.showQuickPick(
      usable.map((provider) => ({
        label: provider.displayName,
        description: provider.providerId === recent ? "Last used" : provider.installation.version ?? "",
        provider,
      })),
      { title: "New conversation", placeHolder: "Choose a coding service" },
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
    access: WorkspaceAccess,
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
    access: WorkspaceAccess,
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
      const opened = await this.runtime.start(
        provider.providerId,
        workspace,
        access,
        model,
        reasoningEffort,
        permission,
      );
      await this.refresh();
      await this.select(opened.sessionId, follow);
      return opened.sessionId;
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
      await vscode.window.showErrorMessage(`Runtrol: ${sentence}`);
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
  /// sidebar's problem row so a person does not have to attempt a conversation just to be told how to fix
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

  async prompt(text?: string): Promise<void> {
    const session = this.requireSelected();
    const written = text ?? await vscode.window.showInputBox({
      title: `Prompt ${path.basename(session.workspace)}`,
      prompt: "The text is sent unchanged to the provider CLI.",
      ignoreFocusOut: true,
    });
    if (!written?.trim()) {
      return;
    }
    await this.runtime.submitInput(runtimeAction(session), written);
  }

  async submitResolvedInput(sessionId: string, text: string): Promise<void> {
    await this.refresh();
    const session = this.state.sessions.find((candidate) => candidate.sessionId === sessionId);
    if (!session) {
      throw new Error("the Mission session is no longer listed by Runtime");
    }
    await this.runtime.submitInput(runtimeAction(session), text);
  }

  async interrupt(): Promise<void> {
    const session = this.requireSelected();
    await this.runtime.interrupt(runtimeAction(session));
  }

  async openConversation(): Promise<void> {
    await this.conversation.show();
    this.ensureSelectedWatch();
  }

  async revealConversationOnEntry(): Promise<void> {
    if (this.conversation.isOpen) {
      this.ensureSelectedWatch();
      return;
    }
    await this.conversation.show(true);
    this.ensureSelectedWatch();
  }

  conversationVisibilityChanged(visible: boolean): void {
    this.conversationVisible = visible;
    if (visible) {
      // Visibility only turns true right after the view reset a freshly (re)born document
      // (retainContextWhenHidden is false, so a hidden tab always comes back empty). Resuming from
      // the stored cursor painted nothing into that empty page; replaying the daemon's bounded
      // window is what brings the conversation back.
      const selected = this.state.selected;
      if (selected) {
        this.state.forgetCursor(selected.sessionId);
      }
      this.ensureSelectedWatch();
    } else {
      this.pauseWatch();
    }
  }

  async nameSession(value?: ConversationItem | SessionLine): Promise<void> {
    const session = this.sessionOf(value);
    const label = await vscode.window.showInputBox({
      title: `Rename ${sessionTitle(session)}`,
      prompt: "Use a short name for this chat. Leave it empty to restore the automatic name.",
      value: session.label ?? "",
      placeHolder: sessionTitle({ ...session, label: null }),
      ignoreFocusOut: true,
      validateInput: validateSessionLabel,
    });
    if (label === undefined) {
      return;
    }
    const normalized = label.trim() || null;
    const { response } = await this.client.once({
      ask: "rename",
      with: { session: session.sessionId, label: normalized },
    });
    requireDone(response, "rename");
    await this.refresh();
    const renamed = this.state.sessions.find((candidate) => candidate.sessionId === session.sessionId);
    if (renamed && this.state.selected?.sessionId === renamed.sessionId) {
      this.conversation.updateSession(renamed);
    }
  }

  async answerApproval(approval: string, option: number, subjectDigest: number[]): Promise<void> {
    const session = this.requireSelected();
    await this.runtime.answerApproval(
      runtimeAction(session),
      approval,
      option,
      subjectDigest,
    );
  }

  async close(value?: ConversationItem | SessionLine): Promise<void> {
    const session = this.sessionOf(value);
    const action = session.lifecycle === "hotRunning" ? "Stop and close" : "Close in Runtrol";
    const choice = await vscode.window.showWarningMessage(
      `Close the ${session.providerId} chat in ${path.basename(session.workspace)}?`,
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
    await this.runtime.close(runtimeAction(session), interruptRunning);
    await this.refresh();
    if (!this.state.selected) {
      this.pauseWatch();
      this.conversation.reset(null);
    }
  }

  async openWorkspace(value?: ConversationItem | SessionLine): Promise<void> {
    const session = this.sessionOf(value);
    await this.persistSelection(session.sessionId);
    if (workspaceIsOpen(session.workspace)) {
      await vscode.commands.executeCommand("workbench.view.explorer");
      return;
    }
    await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(session.workspace), {
      forceNewWindow: false,
    });
  }

  dispose(): void {
    this.disposed = true;
    this.watchAbort?.abort();
    this.indexAbort?.abort();
    this.cancelNativeDiscoveries();
    this.client.dispose();
    this.runtime.dispose();
  }

  selectedWatchReady(): Promise<void> {
    return this.watchReady;
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

  private startWatch(session: SessionLine): void {
    const abort = new AbortController();
    this.watchAbort = abort;
    let ready = () => {};
    this.watchReady = new Promise<void>((resolve) => {
      ready = resolve;
    });
    void this.watchLoop(session, abort.signal, ready);
  }

  private ensureSelectedWatch(): void {
    const selected = this.state.selected;
    if (!this.conversationVisible || !selected || this.watchAbort) {
      return;
    }
    this.startWatch(selected);
  }

  private pauseWatch(): void {
    this.watchAbort?.abort();
    this.watchAbort = null;
    this.watchReady = Promise.resolve();
  }

  private async watchLoop(session: SessionLine, signal: AbortSignal, ready: () => void): Promise<void> {
    let retryMs = 250;
    while (!signal.aborted && !this.disposed && this.state.selected?.sessionId === session.sessionId) {
      try {
        await this.runtime.watchEvents(
          session.sessionId,
          this.state.cursor(session.sessionId),
          {
            started: ready,
            event: (payload, nextExpected) => {
              if (this.conversation.frame(payload)) {
                this.state.advance(session.sessionId, nextExpected);
                return true;
              } else {
                this.pauseWatch();
                return false;
              }
            },
            gap: (nextExpected, message) => {
              this.state.advance(session.sessionId, nextExpected);
              this.conversation.status(message, "warning");
            },
          },
          signal,
        );
        retryMs = 250;
      } catch (error) {
        if (signal.aborted) {
          return;
        }
        this.conversation.status(error instanceof Error ? error.message : String(error), "error");
      }
      await abortableDelay(retryMs, signal);
      retryMs = Math.min(retryMs * 2, 5_000);
    }
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
          ),
        ]).finally(() => connected.abort());
        retryMs = 250;
      } catch (error) {
        if (signal.aborted) {
          return;
        }
        this.conversation.status(error instanceof Error ? error.message : String(error), "error");
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
    const previousSelected = this.state.selected;
    const selected = previousSelected?.sessionId ?? null;
    this.state.replace(sessions, providers);
    const currentSelected = this.state.selected;
    if (currentSelected && currentSelected !== previousSelected) {
      this.conversation.updateSession(currentSelected);
    }
    for (const warning of warnings) {
      if (!this.seenWarnings.has(warning)) {
        this.seenWarnings.add(warning);
        this.conversation.status(warning, "warning");
      }
    }
    if (selected && !this.state.selected) {
      this.pauseWatch();
      void this.clearPersistedSelection().catch((error: unknown) => {
        this.conversation.status(
          `Cannot clear the selected session: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      });
      this.conversation.reset(null);
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
            this.conversation.status(
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
    if (active) return active.pending;
    if (!force && this.state.nativeCatalogue(providerId)) return Promise.resolve();
    const provider = this.state.providers.find((candidate) => candidate.providerId === providerId);
    if (!provider || !isUsable(provider)) return Promise.resolve();
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
        this.state.setNativeCatalogue(nativeCatalogueFailure(providerId, error));
      }
    }).finally(() => {
      if (this.nativeDiscoveries.get(providerId)?.pending === pending) {
        this.nativeDiscoveries.delete(providerId);
      }
    });
    this.nativeDiscoveries.set(providerId, { abort, pending });
    return pending;
  }

  private cancelNativeDiscoveries(): string[] {
    this.nativeDiscoveryGeneration += 1;
    if (this.nativeDiscoveryRestart) {
      clearTimeout(this.nativeDiscoveryRestart);
      this.nativeDiscoveryRestart = null;
    }
    const providerIds = new Set([
      ...this.nativeDiscoveries.keys(),
      ...this.deferredNativeProviders,
    ]);
    this.deferredNativeProviders.clear();
    for (const discovery of this.nativeDiscoveries.values()) {
      discovery.abort.abort(new Error("foreground chat action has priority"));
    }
    this.nativeDiscoveries.clear();
    return [...providerIds];
  }

  private beginForegroundAction(): string[] {
    this.nativeDiscoveryPauseDepth += 1;
    return this.cancelNativeDiscoveries();
  }

  private endForegroundAction(providerIds: readonly string[]): void {
    for (const providerId of providerIds) this.deferredNativeProviders.add(providerId);
    this.nativeDiscoveryPauseDepth = Math.max(0, this.nativeDiscoveryPauseDepth - 1);
    this.scheduleNativeDiscoveries();
  }

  private startExistingChatDiscovery(): void {
    for (const provider of this.state.providers.filter(isUsable)) {
      this.deferredNativeProviders.add(provider.providerId);
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
    const providerIds = [...this.deferredNativeProviders];
    this.deferredNativeProviders.clear();
    for (const providerId of providerIds) this.discoverNativeChats(providerId);
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
  async switchModel(available: readonly string[]): Promise<void> {
    const session = this.state.selected;
    if (!session) return;
    let model: string | null = null;
    let effort: string | null = null;
    if (available.length > 0) {
      const picked = await vscode.window.showQuickPick(
        available.map((id) => ({ label: id })),
        {
          title: "Switch model",
          placeHolder: "Models this conversation says it can switch to",
        },
      );
      if (!picked) return;
      model = picked.label;
    } else {
      const catalogue = await this.runtime.models(session.providerId);
      const choices = modelOptions(catalogue);
      if (choices.length === 0) {
        this.conversation.status(
          `${providerDisplayName(session.providerId, this.state.providers)} reports no switchable models; its own settings stay in control.`,
          "info",
        );
        return;
      }
      const picked = await vscode.window.showQuickPick(choices, {
        title: "Switch model",
        placeHolder: "Choose a model reported by the installed CLI",
      });
      if (!picked) return;
      model = picked.id;
      const pickedEffort = await this.pickReasoningEffort(
        catalogue,
        picked.model,
        "Switch model: reasoning effort",
      );
      if (pickedEffort === undefined) return;
      effort = pickedEffort;
    }
    await this.switchSelectedModel(model, effort ?? undefined);
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
    this.conversation.switchRequested("model", model);
    if (reasoningEffort !== undefined) {
      this.conversation.switchRequested("effort", reasoningEffort);
    }
  }

  /// Switch only the reasoning effort, keeping the model the provider says is answering.
  ///
  /// `sessions/setModel` is the one switch surface and it requires the model, so the page reports
  /// which one is currently answering. A conversation whose provider never announced an answering
  /// model has nothing true to attach an effort to, and this says so instead of guessing one.
  async switchEffort(currentModel: string): Promise<void> {
    const session = this.state.selected;
    if (!session) return;
    if (!currentModel) {
      this.conversation.status(
        `${providerDisplayName(session.providerId, this.state.providers)} has not announced which model is answering, so the effort has nothing to attach to; its own settings stay in control.`,
        "info",
      );
      return;
    }
    const catalogue = await this.runtime.models(session.providerId);
    const model = modelOptions(catalogue).find((option) => option.id === currentModel)?.model ?? null;
    const efforts = reasoningOptions(catalogue, model);
    if (efforts.length === 0) {
      this.conversation.status(
        `${providerDisplayName(session.providerId, this.state.providers)} reports no reasoning efforts for ${currentModel}; its own settings stay in control.`,
        "info",
      );
      return;
    }
    const picked = await this.pickReasoningEffort(catalogue, model, "Switch reasoning effort");
    if (picked === undefined) return;
    await this.runtime.setModel(runtimeAction(session), currentModel, picked ?? undefined);
    this.conversation.switchRequested("effort", picked ?? "default");
  }

  /// Switch the governing permission mode of the open conversation, from its own header chip.
  ///
  /// The choices are the session's own announced set when the protocol gave one, and the service's
  /// manifest-declared switchable set otherwise (the same boundary the daemon enforces, so nothing offered
  /// here can be refused there as out of vocabulary). A service with neither keeps mode in its own surface,
  /// and this says so instead of inventing a picker with nothing true to offer.
  async switchMode(available: readonly string[]): Promise<void> {
    const session = this.state.selected;
    if (!session) return;
    const provider = this.state.providers.find(
      (candidate) => candidate.providerId === session.providerId,
    );
    const choices = available.length > 0 ? available : (provider?.switchableModes ?? []);
    if (choices.length === 0) {
      this.conversation.status(
        `${providerDisplayName(session.providerId, this.state.providers)} announces no switchable modes; its own surface stays in control.`,
        "info",
      );
      return;
    }
    const picked = await vscode.window.showQuickPick(
      choices.map((id) => ({ label: id })),
      {
        title: "Switch mode",
        placeHolder: "Modes this service accepts a switch to",
      },
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
    this.conversation.switchRequested("mode", mode);
  }

  /// One effort picker for every path that asks: `undefined` is a cancel, `null` is the provider's default.
  private async pickReasoningEffort(
    catalogue: ModelCatalog,
    model: ModelOption["model"],
    title: string,
    preferred: string | null = null,
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
    const picked = await vscode.window.showQuickPick(
      [
        {
          label: "Provider default",
          id: null,
          description: "Use the installed CLI's current effort setting",
        },
        ...effortChoices,
      ],
      {
        title,
        placeHolder: "Choose an effort reported for this model",
      },
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
  ): Promise<StartWorkspace["access"] | "another" | null> {
    const collisions = workspaceCollisions(workspace, this.state.sessions);
    if (collisions.length === 0) return "exclusive";
    const buttons = alternatives === "offer"
      ? ["Focus existing", "Choose another", "Start here anyway"]
      : ["Focus existing", "Start here anyway"];
    const action = await vscode.window.showWarningMessage(
      `${path.basename(workspace)} overlaps ${collisions.length} running chat${
        collisions.length === 1 ? "" : "s"
      }.`,
      {
        modal: true,
        detail: collisionDetail(collisions),
      },
      ...buttons,
    );
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
    this.status.text = selected
      ? `$(pulse) ${path.basename(selected.workspace)}  ${hot}/${this.state.sessions.length}`
      : `$(pulse) Runtrol  ${hot}/${this.state.sessions.length}`;
    this.status.tooltip = `${hot} running conversations, ${this.state.sessions.length} total`;
    this.status.command = "runtrol.switchSession";
    this.status.backgroundColor = undefined;
  }
}


type StartWorkspace = {
  workspace: string;
  access: WorkspaceAccess;
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

function requireDone(response: Response, operation: string): void {
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  if (response.say !== "done") {
    throw new Error(`the daemon answered ${operation} with ${response.say}`);
  }
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
