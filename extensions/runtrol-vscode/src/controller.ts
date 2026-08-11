import path from "node:path";

import * as vscode from "vscode";

import { ConversationView } from "./conversationView";
import { CoreClient } from "./core/client";
import type {
  ModelCatalog,
  ModelChoice,
  ProviderLine,
  Response,
  SessionLine,
  WorkspaceAccess,
} from "./protocol";
import { SelectionStore } from "./selectionStore";
import { RuntimeState } from "./state";
import { SessionItem } from "./trees";
import { workspaceCollisions, type WorkspaceCollision } from "./workspaceCollision";

export class Controller implements vscode.Disposable {
  private watchAbort: AbortController | null = null;
  private indexAbort: AbortController | null = null;
  private readonly status: vscode.StatusBarItem;
  private selectionTail: Promise<void> = Promise.resolve();
  private watchReady: Promise<void> = Promise.resolve();
  private disposed = false;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: CoreClient,
    private readonly state: RuntimeState,
    private readonly conversation: ConversationView,
    private readonly selection: SelectionStore,
  ) {
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 20);
    this.status.name = "Runtrol sessions";
    this.status.command = "runtrol.sessions.focus";
    this.status.show();
    context.subscriptions.push(this.status, state.onDidChange(() => this.updateStatus()));
  }

  async initialize(): Promise<void> {
    await this.refresh();
    this.startSessionIndexWatch();
    const remembered = await this.selection.load();
    const selected = this.state.sessions.find((session) => session.session === remembered)
      ?? this.state.sessions.find((session) => session.hot)
      ?? null;
    if (selected) {
      await this.select(selected, false);
    } else {
      this.conversation.reset(null);
    }
  }

  async refresh(): Promise<void> {
    const { response, providers } = await this.client.once({ ask: "list" });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "sessions") {
      throw new Error(`the daemon answered list with ${response.say}`);
    }
    this.applyListing(response.with.sessions, response.with.warnings, providers);
  }

  async reconnect(): Promise<void> {
    this.watchAbort?.abort();
    this.watchAbort = null;
    this.indexAbort?.abort();
    this.indexAbort = null;
    await this.client.reset();
    await this.refresh();
    this.startSessionIndexWatch();
    const selected = this.state.selected;
    if (selected) {
      this.conversation.reset(selected);
      this.startWatch(selected);
    } else {
      this.conversation.reset(null);
    }
  }

  select(value: SessionItem | SessionLine | string, follow = true): Promise<void> {
    const selected = this.selectionTail.then(() => this.selectNow(value, follow));
    this.selectionTail = selected.catch(() => undefined);
    return selected;
  }

  private async selectNow(value: SessionItem | SessionLine | string, follow: boolean): Promise<void> {
    const id = typeof value === "string" ? value : value instanceof SessionItem ? value.session.session : value.session;
    const session = this.state.sessions.find((candidate) => candidate.session === id);
    if (!session) {
      throw new Error("that session is no longer listed");
    }
    await this.selection.save(session.session);
    this.watchAbort?.abort();
    this.state.select(session.session);
    this.conversation.reset(session);

    const follows = vscode.workspace.getConfiguration("runtrol").get<boolean>("followWorkspace", true);
    if (follow && follows && !workspaceIsOpen(session.workspace)) {
      await this.openWorkspace(session);
      return;
    }
    this.startWatch(session);
  }

  async startSession(): Promise<void> {
    await this.refresh();
    const selectedWorkspace = await this.chooseStartWorkspace();
    if (!selectedWorkspace) {
      return;
    }
    const provider = await chooseProvider(this.state.providers);
    if (!provider) {
      return;
    }
    const model = await this.chooseModel(provider);
    if (model === undefined) {
      return;
    }
    const { response } = await this.client.once({
      ask: "start",
      with: {
        provider: provider.id,
        workspace: selectedWorkspace.workspace,
        workspace_access: selectedWorkspace.access,
        model,
        permission: null,
      },
    });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "started") {
      throw new Error(`the daemon answered start with ${response.say}`);
    }
    await this.refresh();
    await this.select(response.with.session);
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
    const { response } = await this.client.once({
      ask: "prompt",
      with: { session: session.session, text: written },
    });
    requireDone(response, "prompt");
  }

  async interrupt(): Promise<void> {
    const session = this.requireSelected();
    const { response } = await this.client.once({
      ask: "interrupt",
      with: { session: session.session },
    });
    requireDone(response, "interrupt");
  }

  async answerApproval(approval: string, option: number, subjectDigest: number[]): Promise<void> {
    const session = this.requireSelected();
    const { response } = await this.client.once({
      ask: "answerApproval",
      with: {
        session: session.session,
        approval,
        option,
        subject_digest: subjectDigest,
      },
    });
    requireDone(response, "answer approval");
  }

  async close(value?: SessionItem | SessionLine): Promise<void> {
    const session = value instanceof SessionItem ? value.session : value ?? this.requireSelected();
    const choice = await vscode.window.showWarningMessage(
      `Close the ${session.provider} session in ${path.basename(session.workspace)}?`,
      { modal: true },
      "Close gracefully",
      "Stop now",
    );
    if (!choice) {
      return;
    }
    const { response } = await this.client.once({
      ask: "close",
      with: { session: session.session, now: choice === "Stop now" },
    });
    requireDone(response, "close");
    await this.refresh();
    if (!this.state.selected) {
      this.watchAbort?.abort();
      this.conversation.reset(null);
    }
  }

  async openWorkspace(value?: SessionItem | SessionLine): Promise<void> {
    const session = value instanceof SessionItem ? value.session : value ?? this.requireSelected();
    await this.selection.save(session.session);
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

  private async watchLoop(session: SessionLine, signal: AbortSignal, ready: () => void): Promise<void> {
    let retryMs = 250;
    while (!signal.aborted && !this.disposed && this.state.selected?.session === session.session) {
      try {
        await this.client.watch(
          session.session,
          this.state.cursor(session.session),
          {
            started: ready,
            event: (payload, nextExpected) => {
              this.state.advance(session.session, nextExpected);
              this.conversation.frame(payload);
            },
            gap: (nextExpected, message) => {
              this.state.advance(session.session, nextExpected);
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

  private async sessionIndexLoop(signal: AbortSignal): Promise<void> {
    let retryMs = 250;
    while (!signal.aborted && !this.disposed) {
      try {
        await this.client.watchSessions(
          {
            snapshot: (listing, providers) => this.applyListing(listing.sessions, listing.warnings, providers),
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

  private applyListing(
    sessions: readonly SessionLine[],
    warnings: readonly string[],
    providers: readonly ProviderLine[],
  ): void {
    const selected = this.state.selected?.session ?? null;
    this.state.replace(sessions, providers);
    for (const warning of warnings) {
      this.conversation.status(warning, "warning");
    }
    if (selected && !this.state.selected) {
      this.watchAbort?.abort();
      this.watchReady = Promise.resolve();
      void this.selection.clear().catch((error: unknown) => {
        this.conversation.status(
          `Cannot clear the selected session: ${error instanceof Error ? error.message : String(error)}`,
          "warning",
        );
      });
      this.conversation.reset(null);
    }
  }

  private async chooseModel(provider: ProviderLine): Promise<string | null | undefined> {
    const { response } = await this.client.once({ ask: "models", with: { provider: provider.id } });
    if (response.say === "failed") {
      throw new Error(response.with.message);
    }
    if (response.say !== "models") {
      throw new Error(`the daemon answered models with ${response.say}`);
    }
    const choices = modelChoices(response.with);
    if (choices.length === 0) {
      return null;
    }
    const selected = await vscode.window.showQuickPick(
      [{ label: "Provider default", id: null, description: "Let the installed CLI choose" }, ...choices],
      { title: `Model for ${provider.display_name}`, placeHolder: "Select a runtime-discovered model" },
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

type StartWorkspace = {
  workspace: string;
  access: WorkspaceAccess;
};

function requireDone(response: Response, operation: string): void {
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  if (response.say !== "done") {
    throw new Error(`the daemon answered ${operation} with ${response.say}`);
  }
}

async function chooseProvider(providers: readonly ProviderLine[]): Promise<ProviderLine | null> {
  const usable = providers.filter((provider) => provider.usable);
  if (usable.length === 0) {
    throw new Error("no installed coding-agent CLI is currently usable");
  }
  const selected = await vscode.window.showQuickPick(
    usable.map((provider) => ({ label: provider.display_name, description: provider.id, provider })),
    { title: "Start a runtrol session", placeHolder: "Select an installed CLI" },
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
      description: `${session.provider}  ${session.doing}`,
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
    .map(({ session }) => `${session.provider}: ${session.workspace}`)
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
  const models: ModelChoice[] = catalog.kind === "known" || catalog.kind === "partial" ? catalog.models : [];
  const aliases = catalog.kind === "aliases" || catalog.kind === "partial" ? catalog.aliases : [];
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
