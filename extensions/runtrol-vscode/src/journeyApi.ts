import type { MirrorEvidence } from "./windowRegistry";
import * as vscode from "vscode";

import { stopping } from "./conversationList";
import { Controller } from "./controller";
import type { IsolatedWorkspaceLine } from "./protocol";
import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { RuntimeState } from "./state";
import { ConversationItem } from "./sidebarTargets";
import type { JourneyInputTiming, TerminalTabs } from "./terminalTabs";
import { workspaceCollisions } from "./workspaceCollision";

export type JourneyTerminal = {
  runtimeGeneration: string;
  terminalId: string;
  terminalGeneration: number;
  providerId: string;
  workspace: string;
};

export type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  start(
    provider: string,
    workspace: string,
    model?: string | null,
    reasoningEffort?: string | null,
    permission?: string | null,
  ): Promise<string>;
  select(session: string): Promise<void>;
  prompt(text: string): Promise<void>;
  switchModel(model: string): Promise<void>;
  switchMode(mode: string): Promise<void>;
  answerApproval(approval: string, option: number, subjectDigest: number[]): Promise<void>;
  interrupt(): Promise<void>;
  reconnect(): Promise<void>;
  openWorkspace(session: string): Promise<void>;
  close(session: string, now?: boolean): Promise<void>;
  verifySelected(session: string): Promise<void>;
  /// The eye pass: open up to `limit` listed conversations as tabs, in list order, skipping any that refuse
  /// and any whose folder already has a live writer (so no collision question can block a headless run).
  /// Returns how many opened, and why each refusal refused (the harness prints them: a refusal is a fact
  /// about the product, not noise).
  openListed(limit: number): Promise<{ opened: number; refused: string[] }>;
  /// The sidebar row's activity word for a session (the provider's running tool), or null.
  rowTool(session: string): string | null;
  /// The sidebar row's state for a session ("needsYou", "working", ...), or null when it has no row.
  rowActivity(session: string): string | null;
  /// Whether a session's row says a question is pending, and answer it from the row as the inline button would.
  answerFromRow(session: string, how: "allow" | "decline"): Promise<void>;
  /// The previous-project memory the back key reads, for the harness to assert.
  previousProject(): string | null;
  /// Select a session's row in the sidebar (its inline actions show on a selected row).
  revealRow(session: string): Promise<void>;
  /// The project folders remembered for the keyboard switch, for the harness to assert.
  knownProjects(): readonly string[];
  isolationEvidence(): Promise<{
    workspaces: readonly IsolatedWorkspaceLine[];
    roots: readonly string[];
  }>;
  /// Every observed mirror this window opened or was refused, with the digest of what it fed.
  windowMirrors(): MirrorEvidence[];
  /// The provider command names the registry recognises, or null while the inventory has not answered.
  windowCommandNames(): string[] | null;
  /// Click a sidebar row by its key, the way the webview does.
  clickRow(key: string): Promise<void>;
  /// File a folder as a project, the way the "Add a project" action does.
  addProject(folder: string): Promise<void>;
  /// The name of the terminal VS Code shows as active in this window, or null.
  activeTerminalName(): string | null;
  /// This window's registered session identity.
  windowSessionId(): string;
  /// The last refusal the Runtime gave this window's publish, or null while every publish held.
  windowPublishFailure(): string | null;
  /// Exactly what this window's next registry publish would send, for holding against a refusal.
  windowUpdatePayload(): unknown;
  /// The keys of the rows the sidebar lists right now.
  rowKeys(): string[];
  /// Every row as the sidebar holds it right now, reduced to what row identity is judged by: a snapshot the
  /// row-identity eye pass takes many times while a launch promotes from placeholder to terminal to conversation.
  rows(): { key: string; title: string; presence: string; hostedKey: string | null; origin: string | null; ownerWindow: string | null; native: string | null; workspace: string; open: boolean; live: boolean; canOpen: boolean; canFocus: boolean; canStop: boolean; stopping: boolean; blocked: string | null; activity: string }[];
  /// Close a conversation's tab in this window by its row key, the way the tab's close button does.
  closeTab(key: string): boolean;
  /// Stop a hosted conversation's process from its row, the way the row's Stop does after its confirmation.
  stopRow(key: string): Promise<void>;
  setDialogue(key: string, enabled: boolean): Promise<void>;
  /// What the sidebar knows beside its rows: whether the Core answers, the terminal listing's own warnings (the
  /// "why is the list incomplete" answer), and the Runtime's managed session records as this window lists them.
  listing(): { coreReach: string; warnings: string[]; incomplete: string | null; sessions: { sessionId: string; providerId: string; native: string | null; lifecycle: string; hot: boolean; workspace: string }[] };
  /// The `+` button's own path: a placeholder row and its tab at once, the Runtime open still pending. Returns as
  /// soon as the tab exists so a caller can watch the placeholder promote.
  startFresh(providerId: string, workspace: string): Promise<void>;
  /// What one row says it can do, as the click path decides it: the words on the row follow these facts.
  rowFacts(key: string): { live: boolean; canOpen: boolean; canFocus: boolean; blocked: string | null } | null;
  /// What the Runtime answered to this window's last owner reveal, or null.
  lastReveal(): { delivered: boolean; foreground: string } | null;
  /// The last sentence a row click answered with instead of opening anything, or null.
  lastExplanation(): string | null;
  /// Open the newest stored conversation of a service that has a title (a reopened conversation with history),
  /// returning its session id, or null when the service lists none.
  openStoredWithTitle(providerId: string): Promise<string | null>;
  /// The focused sidebar eye pass: open two titled provider-owned chats in one workspace in sequence.
  ///
  /// The second selection deliberately overlaps the first idle provider process. Production selection must cool the
  /// first and switch without an internal writer warning. Returns null when this machine has no honest pair to prove.
  switchStoredPair(providerId: string): Promise<{
    first: string;
    second: string;
    workspace: string;
    firstLifecycle: SessionLine["lifecycle"];
    secondLifecycle: SessionLine["lifecycle"];
  } | null>;
  /// Visible titles exactly as the sidebar projection holds them, for the focused no-Untitled eye assertion.
  conversationTitles(): readonly string[];
  /// How many provider-owned conversations the services have listed so far.
  nativeChatCount(): number;
  /// Whether Runtime discovery reports a provider-owned deletion surface for cleanup.
  canDeleteNative(providerId: string): boolean;
  /// The eye pass: whether a provider-owned conversation with this native identity is currently listed.
  nativeChatListed(providerId: string, nativeSessionId: string): boolean;
  /// The eye pass: delete a provider-owned conversation through the provider, without the modal question
  /// (a headless window cannot answer one). The same relay and the same refresh the row's button uses.
  /// Resolves with how many milliseconds the row stayed listed after the deletion was asked (null when it
  /// was still listed when the provider had answered), which is the number the eye pass reads against its
  /// budget: the row must leave on the click, not on the provider's answer.
  deleteNativeListed(providerId: string, nativeSessionId: string): Promise<number | null>;
  /// The eye pass: pin or unpin a listed conversation, the same local ordering choice the row's pin button
  /// sets, so a pinned conversation sorts to the top of its list.
  pinListed(providerId: string, nativeSessionId: string): Promise<void>;
  /// The eye pass: give a listed conversation a local nickname, the same instant rename the row's pencil sets,
  /// so the row then shows the operator's name instead of the service's own.
  nameListed(providerId: string, nativeSessionId: string, label: string): Promise<void>;
  /// The eye pass: select a listed conversation's row so its inline actions (rename, pin, delete) show for a
  /// photograph, whether or not it has a running session.
  revealConversation(providerId: string, nativeSessionId: string): Promise<void>;
  /// The eye pass: a listed conversation that belongs to a folder, which is the one a heading files. Pinning
  /// one of these is the case that lifts a row out from under a heading, so the pass needs to name one.
  filedConversation(providerId: string): { nativeSessionId: string; title: string } | null;
  /// The eye pass: every identity the sidebar tree currently holds, so the pass can assert no conversation is
  /// drawn in two places at once.
  treeItemIds(): readonly string[];
  /// The eye pass: ask the services for their lists again and wait for the answers.
  refreshChats(): Promise<void>;
  /// Wait until one session reports a lifecycle, or fail at the deadline.
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
  /// Installed-host proof of the actual editor terminal path. These methods are absent from release bundles.
  terminalStart(providerId: string, workspace: string, deadlineMs: number): Promise<JourneyTerminal>;
  terminalAttach(runtimeGeneration: string, terminalId: string, deadlineMs: number): Promise<JourneyTerminal>;
  terminalWaitForOutput(
    runtimeGeneration: string,
    terminalId: string,
    text: string,
    deadlineMs: number,
  ): Promise<number>;
  terminalWrite(runtimeGeneration: string, terminalId: string, text: string): Promise<JourneyInputTiming>;
  terminalWriteDirect(runtimeGeneration: string, terminalId: string, text: string): Promise<JourneyInputTiming>;
  /// Digest the raw output chunks from the one carrying `startText` through the one carrying `endText`.
  terminalRecordOutput(
    runtimeGeneration: string,
    terminalId: string,
    startText: string,
    endText: string,
    deadlineMs: number,
  ): Promise<{ chunks: number; bytes: number; digest: string }>;
  /// The pane's size changed, exactly as VS Code tells the pseudoterminal when its tab is resized.
  terminalSetDimensions(runtimeGeneration: string, terminalId: string, columns: number, rows: number): void;
  terminalStop(runtimeGeneration: string, terminalId: string, deadlineMs: number): Promise<void>;
};

export function journeyApi(
  controller: Controller,
  state: RuntimeState,
  terminals: TerminalTabs,
  afterReady: <T>(action: () => Promise<T>) => Promise<T>,
  extensionMode: vscode.ExtensionMode,
  revealRow: (sessionId: string) => Promise<void> = async () => {},
  revealByKey: (key: string) => Promise<void> = async () => {},
  treeItemIds: () => string[] = () => [],
  windowMirrors: () => MirrorEvidence[] = () => [],
  windowCommandNames: () => string[] | null = () => null,
  addProject: (folder: string) => Promise<void> = async () => {},
  windowPublishFailure: () => string | null = () => null,
  windowUpdatePayload: () => unknown = () => null,
): JourneyApi | undefined {
  if (
    extensionMode !== vscode.ExtensionMode.Test
    || process.env.RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY !== "1"
  ) {
    return undefined;
  }
  return {
    providers: () => [...state.providers],
    sessions: () => [...state.sessions],
    start: (provider, workspace, model = null, reasoningEffort = null, permission = null) => afterReady(
      () => controller.startResolvedSession(provider, workspace, model, reasoningEffort, "exclusive", false, permission),
    ),
    select: (session) => afterReady(() => controller.select(session)),
    // Structured-session tools for the journey: text and settings go to the public Runtime
    // session directly; the terminal tab is the surface for people, not for this harness.
    prompt: (text) => afterReady(async () => {
      const selected = state.selected;
      if (!selected) throw new Error("no session is selected");
      await controller.submitResolvedInput(selected.sessionId, text);
    }),
    switchModel: (model) => afterReady(async () => {
      const selected = state.selected;
      if (!selected) throw new Error("no session is selected");
      await controller.setSelectedModel(selected, model);
    }),
    switchMode: (mode) => afterReady(async () => {
      const selected = state.selected;
      if (!selected) throw new Error("no session is selected");
      await controller.setSelectedMode(selected, mode);
    }),
    answerApproval: (approval, option, subjectDigest) => afterReady(
      () => controller.answerApproval(approval, option, subjectDigest),
    ),
    interrupt: () => afterReady(() => controller.interrupt()),
    reconnect: () => afterReady(() => controller.reconnect()),
    openWorkspace: (session) => afterReady(async () => {
      const selected = state.sessions.find((candidate) => candidate.sessionId === session);
      if (!selected) {
        throw new Error("that session is no longer listed");
      }
      await controller.openWorkspace(selected);
    }),
    close: (session, now = false) => afterReady(() => controller.closeResolvedSession(session, now)),
    verifySelected: (session) => afterReady(async () => {
      if (state.selected?.sessionId !== session) {
        throw new Error(`selected ${state.selected?.sessionId ?? "no session"}, expected ${session}`);
      }
    }),
    openListed: (limit) => afterReady(async () => {
      let opened = 0;
      const refused: string[] = [];
      for (const row of state.conversations) {
        if (opened >= limit) break;
        if (!row.canOpen || row.open || row.projectless) continue;
        if (!row.session && workspaceCollisions(row.workspace, state.sessions).length > 0) continue;
        try {
          await within(controller.select(row), 30_000);
          opened += 1;
        } catch (error) {
          // A conversation its service will not reopen right now is skipped, not fatal: the eye pass wants
          // tabs on screen, and the next row is as good a tab as this one. The reason is kept and printed.
          refused.push(`${row.serviceName} ${row.title}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
      return { opened, refused };
    }),
    rowTool: (session) => state.conversationOf(session)?.tool ?? null,
    rowActivity: (session) => state.conversationOf(session)?.activity ?? null,
    answerFromRow: (session, how) => afterReady(async () => {
      const line = state.sessions.find((candidate) => candidate.sessionId === session);
      if (!line) throw new Error("that session is not listed");
      await controller.answerFromRow(line, how);
    }),
    previousProject: () => controller.previousProjectForJourney(),
    revealRow: (session) => afterReady(() => revealRow(session)),
    knownProjects: () => controller.knownProjectsForJourney(),
    isolationEvidence: () => afterReady(() => controller.isolatedWorkspaceEvidenceForJourney()),
    openStoredWithTitle: (providerId) => afterReady(async () => {
      // A stored conversation opens as its service's terminal; there is no managed session to wait for. The
      // first titled row the tree would show is the one a person would click.
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.canOpen
          && !candidate.open
          && !candidate.projectless
          && candidate.native !== null
          && candidate.native.title !== null,
      );
      if (!row) return null;
      await within(controller.select(row), 30_000);
      return row.key;
    }),
    switchStoredPair: (providerId) => afterReady(async () => {
      const candidates = state.conversations.filter(
        (candidate) => candidate.providerId === providerId
          && candidate.canOpen
          && !candidate.open
          && !candidate.projectless
          && candidate.session === null
          && candidate.native !== null
          && candidate.native.title !== null,
      );
      const byWorkspace = new Map<string, typeof candidates>();
      for (const candidate of candidates) {
        const grouped = byWorkspace.get(candidate.workspace) ?? [];
        grouped.push(candidate);
        byWorkspace.set(candidate.workspace, grouped);
      }
      for (const [workspace, rows] of byWorkspace) {
        if (rows.length < 2 || workspaceCollisions(workspace, state.sessions).length > 0) continue;
        const firstRow = rows[0];
        const secondRow = rows[1];
        if (!firstRow || !secondRow) continue;
        try {
          await within(controller.select(firstRow), 30_000);
          const first = state.sessions.find(
            (session) => session.nativeSessionId === firstRow.native?.nativeSessionId,
          );
          if (!first || first.lifecycle !== "hotIdle") continue;
          await within(controller.select(secondRow), 30_000);
          const currentFirst = state.sessions.find((session) => session.sessionId === first.sessionId);
          const second = state.sessions.find(
            (session) => session.nativeSessionId === secondRow.native?.nativeSessionId,
          );
          if (!currentFirst || !second) continue;
          return {
            first: currentFirst.sessionId,
            second: second.sessionId,
            workspace,
            firstLifecycle: currentFirst.lifecycle,
            secondLifecycle: second.lifecycle,
          };
        } catch {
          // A historical provider row may no longer be resumable. The eye pass needs one real honest pair and walks
          // the bounded workspace groups rather than treating an unrelated stale row as the product verdict.
        }
      }
      return null;
    }),
    conversationTitles: () => state.conversations.map((conversation) => conversation.title),
    nativeChatCount: () => state.nativeChats.length,
    canDeleteNative: (providerId) => (
      state.providerCapabilities(providerId)?.nativeSessionDelete?.availability === "available"
    ),
    nativeChatListed: (providerId, nativeSessionId) => state.nativeChats.some(
      (chat) => chat.providerId === providerId && chat.nativeSessionId === nativeSessionId,
    ),
    deleteNativeListed: (providerId, nativeSessionId) => afterReady(async () => {
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.native?.nativeSessionId === nativeSessionId
          && candidate.session === null,
      );
      if (!row?.native) throw new Error("that provider-owned conversation is not listed");
      const listed = () => state.conversations.some(
        (candidate) => candidate.providerId === providerId
          && candidate.native?.nativeSessionId === nativeSessionId,
      );
      const asked = performance.now();
      const deletion = controller.deleteNativeWithoutAsking(row);
      // The row leaves synchronously on the click; this reads the clock right after it, before the
      // provider has answered anything.
      const goneMs = listed() ? null : performance.now() - asked;
      await deletion;
      return goneMs ?? (listed() ? null : performance.now() - asked);
    }),
    pinListed: (providerId, nativeSessionId) => afterReady(async () => {
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.native?.nativeSessionId === nativeSessionId,
      );
      if (!row) throw new Error("that conversation is not listed");
      await controller.togglePin(row);
    }),
    nameListed: (providerId, nativeSessionId, label) => afterReady(async () => {
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.native?.nativeSessionId === nativeSessionId,
      );
      if (!row) throw new Error("that conversation is not listed");
      await controller.renameConversation(row, label);
    }),
    revealConversation: (providerId, nativeSessionId) => afterReady(async () => {
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.native?.nativeSessionId === nativeSessionId,
      );
      if (!row) throw new Error("that conversation is not listed");
      await revealByKey(row.key);
    }),
    filedConversation: (providerId) => {
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && !candidate.projectless
          && !candidate.pinned
          && candidate.native !== null,
      );
      return row?.native
        ? { nativeSessionId: row.native.nativeSessionId, title: row.title }
        : null;
    },
    treeItemIds: () => treeItemIds(),
    refreshChats: () => afterReady(() => controller.refreshChats()),
    waitForLifecycle: (session, lifecycle, deadlineMs) => afterReady(() => new Promise<void>((resolve, reject) => {
      const matches = (): boolean =>
        state.sessions.some((candidate) => candidate.sessionId === session && candidate.lifecycle === lifecycle);
      if (matches()) {
        resolve();
        return;
      }
      const timer = setTimeout(() => {
        subscription.dispose();
        const current = state.sessions.find((candidate) => candidate.sessionId === session);
        reject(new Error(
          `session ${session} did not reach ${lifecycle} within ${deadlineMs} ms (now ${current?.lifecycle ?? "absent"})`,
        ));
      }, deadlineMs);
      const subscription = state.onDidChange(() => {
        if (!matches()) return;
        clearTimeout(timer);
        subscription.dispose();
        resolve();
      });
    })),
    terminalStart: (providerId, workspace, deadlineMs) => afterReady(async () => {
      await controller.startSessionWith(providerId, workspace);
      return terminals.waitForJourneyTerminal({ providerId, workspace }, deadlineMs);
    }),
    terminalAttach: (runtimeGeneration, terminalId, deadlineMs) => afterReady(async () => {
      const row = await waitForHostedConversation(state, runtimeGeneration, terminalId, deadlineMs);
      await controller.select(row);
      return terminals.waitForJourneyTerminal({ runtimeGeneration, terminalId }, deadlineMs);
    }),
    terminalWaitForOutput: (runtimeGeneration, terminalId, text, deadlineMs) =>
      terminals.waitForJourneyOutput(runtimeGeneration, terminalId, text, deadlineMs),
    terminalWrite: (runtimeGeneration, terminalId, text) =>
      terminals.writeJourneyInput(runtimeGeneration, terminalId, text),
    terminalWriteDirect: (runtimeGeneration, terminalId, text) =>
      terminals.writeDirectJourneyInput(runtimeGeneration, terminalId, text),
    terminalRecordOutput: (runtimeGeneration, terminalId, startText, endText, deadlineMs) =>
      terminals.recordJourneyOutput(runtimeGeneration, terminalId, startText, endText, deadlineMs),
    terminalSetDimensions: (runtimeGeneration, terminalId, columns, rows) =>
      terminals.setJourneyDimensions(runtimeGeneration, terminalId, columns, rows),
    windowMirrors,
    windowCommandNames,
    clickRow: (key) => afterReady(async () => {
      const row = state.conversations.find((candidate) => candidate.key === key);
      if (!row) throw new Error(`no sidebar row has key ${key}`);
      await controller.select(row);
    }),
    addProject,
    activeTerminalName: () => vscode.window.activeTerminal?.name ?? null,
    windowSessionId: () => vscode.env.sessionId,
    windowPublishFailure: () => windowPublishFailure(),
    windowUpdatePayload: () => windowUpdatePayload(),
    rowKeys: () => state.conversations.map((row) => row.key),
    rows: () => state.conversations.map((row) => ({
      key: row.key,
      title: row.title,
      presence: row.presence.kind,
      hostedKey: row.hostedKey,
      origin: row.hostedTerminal?.origin ?? null,
      ownerWindow: row.hostedTerminal?.ownerWindowSessionId ?? null,
      native: row.native?.nativeSessionId ?? null,
      workspace: row.workspace,
      // Open as the sidebar decides it: a tab filed under the row's own key or under the terminal it claims.
      open: terminals.isOpen(row.key) || (row.hostedKey !== null && terminals.isOpen(row.hostedKey)),
      live: row.live,
      canOpen: row.canOpen,
      canFocus: row.canFocus,
      canStop: row.canStop,
      stopping: stopping(row),
      blocked: row.blocked,
      activity: row.activity,
    })),
    closeTab: (key) => terminals.closeTab(key),
    stopRow: (key) => afterReady(async () => {
      const row = state.conversations.find((candidate) => candidate.key === key);
      if (!row) throw new Error(`no sidebar row has key ${key}`);
      await controller.stopHostedResolved(row);
    }),
    setDialogue: (key, enabled) => afterReady(async () => {
      const row = state.conversations.find((candidate) => candidate.key === key);
      if (!row) throw new Error(`no sidebar row has key ${key}`);
      await controller.setDialogue(new ConversationItem(row), enabled);
    }),
    listing: () => ({
      coreReach: state.coreReach,
      warnings: [...state.listingWarnings],
      incomplete: state.incompleteDiscovery,
      sessions: state.sessions.map((session) => ({
        sessionId: session.sessionId,
        providerId: session.providerId,
        native: session.nativeSessionId ?? null,
        lifecycle: session.lifecycle,
        hot: session.hot,
        workspace: session.workspace,
      })),
    }),
    startFresh: (providerId, workspace) => afterReady(() => controller.startSessionWith(providerId, workspace)),
    rowFacts: (key) => {
      const row = state.conversations.find((candidate) => candidate.key === key);
      return row ? { live: row.live, canOpen: row.canOpen, canFocus: row.canFocus, blocked: row.blocked } : null;
    },
    lastReveal: () => controller.lastReveal,
    lastExplanation: () => controller.lastExplanation,
    terminalStop: (runtimeGeneration, terminalId, _deadlineMs) => afterReady(
      () => terminals.stopJourneyTerminal(runtimeGeneration, terminalId),
    ),
  };
}

function waitForHostedConversation(
  state: RuntimeState,
  runtimeGeneration: string,
  terminalId: string,
  deadlineMs: number,
): Promise<RuntimeState["conversations"][number]> {
  return new Promise((resolve, reject) => {
    const find = (): RuntimeState["conversations"][number] | undefined => state.conversations.find(
      (candidate) => candidate.hostedTerminal?.runtimeGeneration === runtimeGeneration
        && candidate.hostedTerminal.terminalId === terminalId,
    );
    const ready = find();
    if (ready) {
      resolve(ready);
      return;
    }
    const timer = setTimeout(() => {
      subscription.dispose();
      reject(new Error(`terminal ${terminalId} did not enter this VS Code window within ${deadlineMs} ms`));
    }, deadlineMs);
    const subscription = state.onDidChange(() => {
      const found = find();
      if (!found) return;
      clearTimeout(timer);
      subscription.dispose();
      resolve(found);
    });
  });
}

function within<T>(work: Promise<T>, milliseconds: number): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}
