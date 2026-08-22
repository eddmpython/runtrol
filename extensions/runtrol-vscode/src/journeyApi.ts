import * as vscode from "vscode";

import type { ConversationPanels } from "./conversationPanels";
import { Controller } from "./controller";
import type { MissionController } from "./mission/controller";
import type { MissionSnapshot } from "./protocol";
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
  sendFocusedDraft(text: string): Promise<string>;
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
  /// Open the newest stored conversation of a service that has a title (a reopened conversation with history),
  /// returning its session id, or null when the service lists none.
  openStoredWithTitle(providerId: string): Promise<string | null>;
  /// How many provider-owned conversations the services have listed so far.
  nativeChatCount(): number;
  /// The eye pass: whether a provider-owned conversation with this native identity is currently listed.
  nativeChatListed(providerId: string, nativeSessionId: string): boolean;
  /// The eye pass: delete a provider-owned conversation through the provider, without the modal question
  /// (a headless window cannot answer one). The same relay and the same refresh the row's button uses.
  deleteNativeListed(providerId: string, nativeSessionId: string): Promise<void>;
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
    sendFocusedDraft: (text) => afterReady(async () => {
      const binding = conversation.focused();
      if (!binding?.draft) throw new Error("no draft tab is focused");
      await controller.sendDraft(binding, text);
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
      const row = state.conversations.find(
        (candidate) => candidate.providerId === providerId
          && candidate.canOpen
          && !candidate.open
          && !candidate.projectless
          && candidate.session === null
          && candidate.native !== null
          && candidate.native.title !== null
          && workspaceCollisions(candidate.workspace, state.sessions).length === 0,
      );
      if (!row) return null;
      await within(controller.select(row), 30_000);
      const session = state.sessions.find(
        (candidate) => candidate.nativeSessionId === row.native?.nativeSessionId,
      );
      return session?.sessionId ?? null;
    }),
    nativeChatCount: () => state.nativeChats.length,
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
      await controller.deleteNativeWithoutAsking(row);
    }),
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
