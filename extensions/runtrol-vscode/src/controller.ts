import path from "node:path";

import * as vscode from "vscode";

import { ConversationView } from "./conversationView";
import { CoreClient } from "./core/client";
import type {
  ProviderUpdateLine,
  Response,
} from "./protocol";
import type {
  ModelCatalog,
  ModelChoice,
  ProviderLine,
  SessionLine,
  WorkspaceAccess,
} from "./runtimeTypes";
import { SelectionStore } from "./selectionStore";
import { sessionTitle } from "./sessionDisplay";
import { sessionChoices } from "./sessionNavigation";
import { RuntimeState } from "./state";
import { SessionItem } from "./trees";
import { StudioRuntimeClient } from "./runtimeClient";
import { sessionStateLabel } from "./runtimeProjection";
import { workspaceCollisions, type WorkspaceCollision } from "./workspaceCollision";

export class Controller implements vscode.Disposable {
  private watchAbort: AbortController | null = null;
  private indexAbort: AbortController | null = null;
  private readonly status: vscode.StatusBarItem;
  private selectionTail: Promise<void> = Promise.resolve();
  private watchReady: Promise<void> = Promise.resolve();
  private conversationVisible = false;
  private disposed = false;
  private readonly seenWarnings = new Set<string>();
  private readonly verifyingProviders = new Set<string>();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: CoreClient,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly conversation: ConversationView,
    private readonly selection: SelectionStore,
  ) {
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.name = "Runtrol sessions";
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

  async switchSession(): Promise<void> {
    const selected = this.state.selected?.sessionId ?? null;
    const picked = await vscode.window.showQuickPick(
      sessionChoices(this.state.sessions, selected, this.state.providers),
      {
        title: `Switch Runtrol session (${this.state.sessions.length})`,
        placeHolder: "Type a project, provider, state, or workspace",
        matchOnDescription: true,
        matchOnDetail: true,
      },
    );
    if (picked) {
      await this.select(picked.session.sessionId);
    }
  }

  async reconnect(): Promise<void> {
    this.pauseWatch();
    this.indexAbort?.abort();
    this.indexAbort = null;
    await this.client.reset();
    await this.runtime.reset();
    await this.refreshAfterReconnect();
    void this.startSessionIndexWatch();
    const selected = this.state.selected;
    if (selected) {
      this.conversation.reset(selected);
      this.ensureSelectedWatch();
    } else {
      this.conversation.reset(null);
    }
  }

  select(value: SessionItem | SessionLine | string, follow = true, reveal = true): Promise<void> {
    const selected = this.selectionTail.then(() => this.selectNow(value, follow, reveal));
    this.selectionTail = selected.catch(() => undefined);
    return selected;
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
    value: SessionItem | SessionLine | string,
    follow: boolean,
    reveal: boolean,
    afterApplied: () => void = () => undefined,
  ): Promise<void> {
    const id = typeof value === "string" ? value : value instanceof SessionItem
      ? value.session.sessionId
      : value.sessionId;
    let session = this.state.sessions.find((candidate) => candidate.sessionId === id);
    if (!session) {
      throw new Error("that session is no longer listed");
    }
    if (reveal) {
      void this.conversation.show();
    }
    if (!session.hot) {
      this.pauseWatch();
      this.state.select(session.sessionId);
      this.conversation.reset(session);
      this.conversation.status("Resuming the provider-owned session...", "info");
      session = await this.resumeSession(session);
    }
    const stored = this.selection.save(session.sessionId);
    this.pauseWatch();
    this.state.select(session.sessionId);
    this.conversation.reset(session);

    const follows = vscode.workspace.getConfiguration("runtrol").get<boolean>("followWorkspace", true);
    if (follow && follows && !workspaceIsOpen(session.workspace)) {
      await stored;
      await vscode.commands.executeCommand("vscode.openFolder", vscode.Uri.file(session.workspace), {
        forceNewWindow: false,
      });
      return;
    }
    this.ensureSelectedWatch();
    afterApplied();
    await stored;
  }

  private async resumeSession(session: SessionLine): Promise<SessionLine> {
    if (!session.nativeSessionId) {
      throw new Error("that cold session has no provider-owned conversation identifier to resume");
    }
    const opened = await this.runtime.resume(runtimeAction(session), "exclusive");
    await this.refresh();
    const resumed = this.state.sessions.find((candidate) => candidate.sessionId === opened.sessionId);
    if (!resumed) {
      throw new Error("the resumed session is absent from the current session index");
    }
    return resumed;
  }

  async startSession(providerId?: string): Promise<void> {
    await this.refresh();
    const provider = providerId
      ? this.state.providers.find((candidate) => candidate.providerId === providerId) ?? null
      : await chooseProvider(this.state.providers);
    if (!provider) {
      if (providerId) {
        throw new Error(`the installed provider ${providerId} is no longer listed`);
      }
      return;
    }
    if (provider.installation.state !== "usable") {
      throw new Error(`the installed provider ${provider.providerId} is not usable`);
    }
    const selectedWorkspace = await this.chooseStartWorkspace();
    if (!selectedWorkspace) {
      return;
    }
    const model = await this.chooseModel(provider);
    if (model === undefined) {
      return;
    }
    await this.startResolvedSession(
      provider.providerId,
      selectedWorkspace.workspace,
      model,
      selectedWorkspace.access,
      true,
    );
  }

  async startResolvedSession(
    providerId: string,
    workspace: string,
    model: string | null,
    access: WorkspaceAccess,
    follow: boolean,
  ): Promise<string> {
    const provider = this.state.providers.find((candidate) => candidate.providerId === providerId);
    if (provider?.installation.state !== "usable") {
      throw new Error(`the installed provider ${providerId} is not usable`);
    }
    const opened = await this.runtime.start(provider.providerId, workspace, access, model);
    await this.refresh();
    await this.select(opened.sessionId, follow);
    return opened.sessionId;
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

  conversationVisibilityChanged(visible: boolean): void {
    this.conversationVisible = visible;
    if (visible) {
      this.ensureSelectedWatch();
    } else {
      this.pauseWatch();
    }
  }

  async nameSession(value?: SessionItem | SessionLine): Promise<void> {
    const session = value instanceof SessionItem ? value.session : value ?? this.requireSelected();
    const label = await vscode.window.showInputBox({
      title: `Rename ${sessionTitle(session)}`,
      prompt: "Use a short name for this session. Leave it empty to restore the automatic name.",
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

  async close(value?: SessionItem | SessionLine): Promise<void> {
    const session = value instanceof SessionItem ? value.session : value ?? this.requireSelected();
    const action = session.lifecycle === "hotRunning"
      ? "Interrupt and close"
      : session.lifecycle === "cold"
        ? "Forget session"
        : "Close session";
    const choice = await vscode.window.showWarningMessage(
      `Close the ${session.providerId} session in ${path.basename(session.workspace)}?`,
      { modal: true },
      action,
    );
    if (choice !== action) {
      return;
    }
    await this.closeResolvedSession(session, session.lifecycle === "hotRunning");
  }

  async closeResolvedSession(
    value: SessionItem | SessionLine | string,
    interruptRunning: boolean,
  ): Promise<void> {
    const id = typeof value === "string" ? value : value instanceof SessionItem
      ? value.session.sessionId
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

  async openWorkspace(value?: SessionItem | SessionLine): Promise<void> {
    const session = value instanceof SessionItem ? value.session : value ?? this.requireSelected();
    await this.selection.save(session.sessionId);
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
    this.client.dispose();
    this.runtime.dispose();
  }

  selectedWatchReady(): Promise<void> {
    return this.watchReady;
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
    const selected = this.state.selected?.sessionId ?? null;
    this.state.replace(sessions, providers);
    for (const warning of warnings) {
      if (!this.seenWarnings.has(warning)) {
        this.seenWarnings.add(warning);
        this.conversation.status(warning, "warning");
      }
    }
    if (selected && !this.state.selected) {
      this.pauseWatch();
      void this.selection.clear().catch((error: unknown) => {
        this.conversation.status(
          `Cannot clear the selected session: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      });
      this.conversation.reset(null);
    }
  }

  private startProviderVerification(providers: readonly ProviderLine[]): void {
    for (const provider of providers) {
      if (!providerNeedsVerification(provider) || this.verifyingProviders.has(provider.providerId)) {
        continue;
      }
      this.verifyingProviders.add(provider.providerId);
      void this.runtime.verifyProvider(provider.providerId).then(async () => {
        if (!this.disposed) await this.refresh();
      }).catch((error: unknown) => {
        if (!this.disposed) {
          this.conversation.status(
            `Cannot verify ${provider.displayName}: ${error instanceof Error ? error.message : String(error)}`,
            "warning",
          );
        }
      }).finally(() => {
        this.verifyingProviders.delete(provider.providerId);
      });
    }
  }

  private async chooseModel(provider: ProviderLine): Promise<string | null | undefined> {
    const choices = modelChoices(await this.runtime.models(provider.providerId));
    if (choices.length === 0) {
      return null;
    }
    const selected = await vscode.window.showQuickPick(
      [{ label: "Provider default", id: null, description: "Let the installed CLI choose" }, ...choices],
      { title: `Model for ${provider.displayName}`, placeHolder: "Select a runtime-discovered model" },
    );
    return selected?.id;
  }

  private async chooseStartWorkspace(): Promise<StartWorkspace | null> {
    let workspace = await chooseWorkspace();
    while (workspace) {
      const collisions = workspaceCollisions(workspace, this.state.sessions);
      if (collisions.length === 0) {
        return { workspace, access: "exclusive" };
      }
      const action = await vscode.window.showWarningMessage(
        `${path.basename(workspace)} overlaps ${collisions.length} running runtrol session${
          collisions.length === 1 ? "" : "s"
        }.`,
        {
          modal: true,
          detail: collisionDetail(collisions),
        },
        "Focus existing",
        "Choose another",
        "Start here anyway",
      );
      if (action === "Start here anyway") {
        return { workspace, access: "shared" };
      }
      if (action === "Focus existing") {
        const existing = await chooseCollision(collisions);
        if (existing) {
          await this.select(existing);
        }
        return null;
      }
      if (action !== "Choose another") {
        return null;
      }
      workspace = await chooseAlternateWorkspace(workspace, this.state.sessions);
    }
    return null;
  }

  private requireSelected(): SessionLine {
    const selected = this.state.selected;
    if (!selected) {
      throw new Error("select a runtrol session first");
    }
    return selected;
  }

  private updateStatus(): void {
    const hot = this.state.sessions.filter((session) => session.hot).length;
    const selected = this.state.selected;
    this.status.text = selected
      ? `$(pulse) ${path.basename(selected.workspace)}  ${hot}/${this.state.sessions.length}`
      : `$(pulse) Runtrol  ${hot}/${this.state.sessions.length}`;
    this.status.tooltip = `${hot} hot sessions, ${this.state.sessions.length} total`;
  }
}

function providerNeedsVerification(provider: ProviderLine): boolean {
  return provider.installation.state === "unavailable"
    && provider.installation.why === "the installed executable has not completed a verified probe";
}

type StartWorkspace = {
  workspace: string;
  access: WorkspaceAccess;
};

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

async function chooseProvider(providers: readonly ProviderLine[]): Promise<ProviderLine | null> {
  const usable = providers.filter((provider) => provider.installation.state === "usable");
  if (usable.length === 0) {
    throw new Error("no installed coding-agent CLI is currently usable");
  }
  const selected = await vscode.window.showQuickPick(
    usable.map((provider) => ({
      label: provider.displayName,
      description: provider.providerId,
      provider,
    })),
    { title: "Start a Runtrol chat", placeHolder: "Choose an installed service" },
  );
  return selected?.provider ?? null;
}

async function chooseWorkspace(): Promise<string | null> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.length === 1) {
    return folders[0]?.uri.fsPath ?? null;
  }
  if (folders.length > 1) {
    const selected = await vscode.window.showQuickPick(
      folders.map((folder) => ({ label: folder.name, description: folder.uri.fsPath, folder })),
      { title: "Workspace for the new session" },
    );
    return selected?.folder.uri.fsPath ?? null;
  }
  const selected = await vscode.window.showOpenDialog({
    title: "Workspace for the new session",
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

function modelChoices(catalog: ModelCatalog): Array<{ label: string; id: string; description?: string }> {
  const models: ModelChoice[] = catalog.coverage === "known" || catalog.coverage === "partial"
    ? [...catalog.models]
    : [];
  const aliases = catalog.coverage === "aliases" || catalog.coverage === "partial"
    ? catalog.aliases
    : [];
  return [
    ...models.map((model) => ({
      label: model.displayName,
      id: model.id,
      description: model.isDefault ? "Provider default" : model.description,
    })),
    ...aliases
      .filter((alias) => !models.some((model) => model.id === alias))
      .map((alias) => ({ label: alias, id: alias, description: "Provider alias" })),
  ];
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
