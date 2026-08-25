import * as vscode from "vscode";

import type { ConversationPanels } from "./conversationPanels";
import { Controller } from "./controller";
import type { MissionController } from "./mission/controller";
import type { IsolatedWorkspaceLine, MissionSnapshot } from "./protocol";
import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { RuntimeState } from "./state";
import { workspaceCollisions } from "./workspaceCollision";

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
  /// The eye pass: a draft tab on a folder (or on no project at all), with a service preselected.
  openDraft(workspace: string | null, providerId?: string): Promise<void>;
  /// The eye pass: send the focused draft's first message, which starts its conversation in the same tab.
  /// Returns the session the tab became.
  sendFocusedDraft(text: string, parallelPlacement?: "isolated" | "shared"): Promise<string>;
  /// The eye pass: open up to `limit` listed conversations as tabs, in list order, skipping any that refuse
  /// and any whose folder already has a live writer (so no collision question can block a headless run).
  /// Returns how many opened, and why each refusal refused (the harness prints them: a refusal is a fact
  /// about the product, not noise).
  openListed(limit: number): Promise<{ opened: number; refused: string[] }>;
  /// Show a session in one of the window's places: "tab", "panel" (bottom) or "sideBar" (secondary).
  placeConversation(session: string, place: "tab" | "panel" | "sideBar"): Promise<void>;
  /// Spread the open conversation tabs over an editor grid; how many were arranged.
  arrangeGrid(): Promise<{ arranged: number; leftInPlace: number }>;
  /// Open the newest declared change of the focused conversation in the diff editor.
  openLatestDiff(): Promise<void>;
  /// Click a chip of the focused conversation's composer, as a person would; its choices open in the page.
  clickChip(anchor: "project" | "service" | "model" | "effort" | "mode"): Promise<void>;
  /// Add a service to the focused draft's "also ask" set, as choosing it in the service chip's menu would.
  alsoAsk(providerId: string): Promise<void>;
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
  registerMissionGate(gateId: string, program: string, arguments_: string[]): Promise<void>;
  validateMissionFile(file: string): Promise<MissionSnapshot>;
  launchFleet(missionId: string): Promise<string[]>;
  continueMission(
    missionId: string,
    operatorChoiceProvider: string,
  ): Promise<{ snapshot: MissionSnapshot; sessionIds: readonly string[]; verified: number }>;
  continueReadyMissions(
    operatorChoiceProvider: string,
  ): Promise<{ missions: number; sessionIds: readonly string[]; verified: number; remainingReady: number }>;
  armMissionAutoFlight(missionId: string, operatorChoiceProvider: string | null): Promise<void>;
  scheduleMission(
    missionId: string,
    dueUnixMs: number,
    operatorChoiceProvider: string | null,
  ): Promise<MissionSnapshot>;
  autoFlightArmed(missionId: string): boolean;
  autoFlightRetained(missionId: string): boolean;
  refreshMissions(): Promise<void>;
  mission(missionId: string): Promise<MissionSnapshot>;
  verifyMissionTask(missionId: string, taskId: string): Promise<MissionSnapshot>;
  compareMissionResults(missionId: string): Promise<void>;
  reviewMissionLanding(missionId: string, taskId?: string): Promise<void>;
  applyMissionLanding(missionId: string, taskId?: string): Promise<MissionSnapshot>;
};

export function journeyApi(
  controller: Controller,
  missions: MissionController,
  state: RuntimeState,
  conversation: ConversationPanels,
  afterReady: <T>(action: () => Promise<T>) => Promise<T>,
  extensionMode: vscode.ExtensionMode,
  revealRow: (sessionId: string) => Promise<void> = async () => {},
  revealByKey: (key: string) => Promise<void> = async () => {},
  treeItemIds: () => string[] = () => [],
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
    prompt: (text) => afterReady(() => controller.prompt(text)),
    switchModel: (model) => afterReady(() => controller.switchSelectedModel(model)),
    switchMode: (mode) => afterReady(() => controller.switchSelectedMode(mode)),
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
      await controller.selectedWatchReady();
      await conversation.bindingFor(session)?.settled();
    }),
    openDraft: (workspace, providerId) => afterReady(async () => {
      await controller.openDraft({ workspace, providerId: providerId ?? null });
    }),
    sendFocusedDraft: (text, parallelPlacement) => afterReady(async () => {
      const binding = conversation.focused();
      if (!binding?.draft) throw new Error("no draft tab is focused");
      await controller.sendDraft(binding, text, parallelPlacement);
      const session = binding.session;
      if (!session) throw new Error("the draft did not become a session");
      return session.sessionId;
    }),
    openListed: (limit) => afterReady(async () => {
      let opened = 0;
      const refused: string[] = [];
      for (const row of state.conversations) {
        if (opened >= limit) break;
        if (!row.canOpen || row.open || row.projectless) continue;
        if (row.session && conversation.bindingFor(row.session.sessionId)) continue;
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
    placeConversation: (session, place) => afterReady(async () => {
      const line = state.sessions.find((candidate) => candidate.sessionId === session);
      if (!line) throw new Error("that session is not listed");
      await controller.placeConversation(place, line);
      await conversation.bindingFor(session)?.settled();
    }),
    arrangeGrid: () => afterReady(() => controller.arrangeGridForJourney()),
    alsoAsk: (providerId) => afterReady(async () => {
      const binding = conversation.focused();
      if (!binding?.draft) throw new Error("no draft tab is focused");
      await controller.alsoAskForJourney(binding, providerId);
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
    clickChip: (anchor) => afterReady(async () => {
      const binding = conversation.focused();
      if (!binding) throw new Error("no conversation tab is focused");
      binding.view.clickChip(anchor);
    }),
    openLatestDiff: () => afterReady(async () => {
      const binding = conversation.focused();
      if (!binding) throw new Error("no conversation tab is focused");
      binding.view.openLatestDiff();
    }),
    openStoredWithTitle: (providerId) => afterReady(async () => {
      const rows = state.conversations.filter(
        (candidate) => candidate.providerId === providerId
          && candidate.canOpen
          && !candidate.open
          && !candidate.projectless
          && candidate.session === null
          && candidate.native !== null
          && candidate.native.title !== null
          && workspaceCollisions(candidate.workspace, state.sessions).length === 0,
      );
      const refused: string[] = [];
      // Real provider stores outlive projects and other applications may still own a conversation. Either fact can
      // make one perfectly listable row impossible to resume. The eye pass needs one real successful resume, not an
      // arbitrary claim that the first historical row represents the provider, so it walks the bounded catalogue
      // candidates until the provider accepts one and reports the sampled refusals if none work.
      for (const row of rows) {
        try {
          await within(controller.select(row), 30_000);
        } catch (error) {
          refused.push(`${row.title}: ${error instanceof Error ? error.message : String(error)}`);
          continue;
        }
        const session = state.sessions.find(
          (candidate) => candidate.nativeSessionId === row.native?.nativeSessionId,
        );
        if (session) return session.sessionId;
        refused.push(`${row.title}: the provider accepted resume but published no managed session`);
      }
      if (rows.length > 0) {
        throw new Error(
          `none of ${rows.length} titled ${providerId} conversations could be reopened: ${refused.slice(0, 5).join(" | ")}`,
        );
      }
      return null;
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
    registerMissionGate: (gateId, program, arguments_) => afterReady(
      () => missions.registerGateForJourney(gateId, program, arguments_),
    ),
    validateMissionFile: (file) => afterReady(() => missions.validateMissionFile(vscode.Uri.file(file))),
    launchFleet: (missionId) => afterReady(() => missions.launchFleetForJourney(missionId)),
    continueMission: (missionId, operatorChoiceProvider) => afterReady(
      () => missions.continueMissionForJourney(missionId, operatorChoiceProvider),
    ),
    continueReadyMissions: (operatorChoiceProvider) => afterReady(
      () => missions.continueReadyMissionsForJourney(operatorChoiceProvider),
    ),
    armMissionAutoFlight: (missionId, operatorChoiceProvider) => afterReady(
      () => missions.armMissionAutoFlightForJourney(missionId, operatorChoiceProvider),
    ),
    scheduleMission: (missionId, dueUnixMs, operatorChoiceProvider) => afterReady(
      () => missions.scheduleMissionForJourney(missionId, dueUnixMs, operatorChoiceProvider),
    ),
    autoFlightArmed: (missionId) => missions.isAutoFlightArmed(missionId),
    autoFlightRetained: (missionId) => missions.hasAutoFlightRecord(missionId),
    refreshMissions: () => afterReady(() => missions.refresh()),
    mission: (missionId) => afterReady(() => missions.snapshot(missionId)),
    verifyMissionTask: (missionId, taskId) => afterReady(
      () => missions.verifyTaskForJourney(missionId, taskId),
    ),
    compareMissionResults: (missionId) => afterReady(async () => {
      const snapshot = await missions.snapshot(missionId);
      await missions.compareResults({ mission: snapshot.mission });
    }),
    reviewMissionLanding: (missionId, taskId) => afterReady(
      () => missions.reviewMissionLandingForJourney(missionId, taskId),
    ),
    applyMissionLanding: (missionId, taskId) => afterReady(
      () => missions.applyMissionLandingForJourney(missionId, taskId),
    ),
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
  };
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
