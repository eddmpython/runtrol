import path from "node:path";

import * as vscode from "vscode";

import { AgentToolsController, type AgentToolsAction } from "./agentTools";
import { CandidateController } from "./capability/controller";
import { conversations as conversationRows } from "./conversationList";
import { ConversationPanels } from "./conversationPanels";
import { PANEL_VIEW_ID, SIDE_BAR_VIEW_ID } from "./conversationSurface";
import { ActivityWatcher } from "./activityWatch";
import { WatchLifecycleGate } from "./watchLifecycleGate";
import { DiffDocuments } from "./diffDocuments";
import { ConversationView, type WebviewPerformance } from "./conversationView";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import { superviseCoreCurrency } from "./coreCurrencySurface";
import { NO_PROJECT_LABEL, readDraftState } from "./draft";
import { readGitBranch } from "./gitBranch";
import {
  confirmRuntimeForget,
  confirmRuntimeSharedOpen,
  manageIntegrations,
  reviewIntegrationEnrollments,
  reviewRuntimeRequests,
  selfApproveIntegration,
} from "./integrationAdministration";
import { journeyApi, type JourneyApi } from "./journeyApi";
import { MissionController } from "./mission/controller";
import { MissionTree } from "./mission/tree";
import { isProjectless, projectlessRoot } from "./projectlessWorkspace";
import { ProjectStore } from "./projects";
import { isBroken } from "./providerHealth";
import { managePhones, pairPhone, reviewPhonePairings } from "./pairingAdministration";
import type { RemoteConnection } from "./protocol";
import { SelectionStore } from "./selectionStore";
import { ServiceTroubleReported } from "./serviceHelp";
import { providerDisplayName, providerIcon, sessionTitle, workspaceName } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { workspaceCovers, workspaceIdentity } from "./workspaceCollision";
import { installableProviders } from "./usageDisplay";
import { UsageView } from "./usageView";
import { WorkspaceRootFollowing } from "./workspaceRoots";
import { ConversationItem, ConversationsTree, ProjectItem } from "./trees";

declare const RUNTROL_INCLUDE_TEST_JOURNEY: boolean;

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<WebviewPerformance>;
  measureSessionManagement?(
    sessionIds: readonly string[],
    progress?: (stage: string) => void,
  ): Promise<SessionManagementPerformance>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
  hasConversationIn?(folder: string): Promise<boolean>;
  waitForConversationIn?(folder: string, deadlineMs: number): Promise<number>;
  seedProject?(folder: string): Promise<void>;
  openFirstConversation?(): Promise<void>;
  openCrossProjectConversation?(): Promise<void>;
  readonly journey?: JourneyApi;
};

export type SessionManagementPerformance = {
  sessionCount: number;
  hotSessionCount: number;
  coldResumeMs: number;
  sessionSwitchP95Ms: number;
  resumedFrom: string;
  resumedTo: string;
  restoreSession: string;
  restoreWorkspace: string;
};

// Eight hot sessions over five rounds keep nearest-rank p95 from collapsing to the single maximum sample.
const SESSION_SWITCH_ROUNDS = 5;

/// Whether the performance-only measurement surface is on, asked once. One name for one flag.
const MEASURED_HOST = process.env.RUNTROL_VSCODE_PERFORMANCE === "1";

export function activate(context: vscode.ExtensionContext): RuntrolExtensionApi {
  const locator = new CoreLocator(context);
  const client = new CoreClient(locator);
  const agentTools = new AgentToolsController(() => locator.runtimeExecutable());
  let initializationStage = "runtime:bootstrap";
  const runtime = new StudioRuntimeClient(
    context,
    () => locator.runtimeExecutable(),
    () => locator.managedDigest(),
    (pendingId, signature) => selfApproveIntegration(client, pendingId, signature),
    (confirmationId, sessionId) => confirmRuntimeForget(client, confirmationId, sessionId),
    (confirmationId, workspace) => confirmRuntimeSharedOpen(client, confirmationId, workspace),
    testIntegrationRoots(context),
    (stage) => {
      initializationStage = `runtime:${stage}`;
    },
  );
  // Conversations started with no project run in the extension's own scratch folder; the state knows it so
  // every derived row agrees on which conversations are projectless.
  const state = new RuntimeState(projectlessRoot(context.globalStorageUri.fsPath));
  const selection = new SelectionStore(context.globalStorageUri.fsPath);
  let settleReady: ((error?: unknown) => void) | null = null;
  let lifecycle: Promise<void> = new Promise<void>((resolve, reject) => {
    settleReady = (error) => {
      settleReady = null;
      if (error === undefined) {
        resolve();
      } else {
        reject(error);
      }
    };
  });
  let missionLifecycle: Promise<void> = Promise.resolve();
  const afterReady = async <T>(action: () => Promise<T>): Promise<T> => {
    await lifecycle;
    return action();
  };
  const afterMissionReady = async <T>(action: () => Promise<T>): Promise<T> => {
    await lifecycle;
    await missionLifecycle;
    return action();
  };
  let controller: Controller;
  // The operator's own projects. Global state, because the panel manages the whole machine from any window.
  // Built before the controller because a draft's project picker offers them first.
  const projectStore = new ProjectStore(context.globalState);
  const watchLifecycle = new WatchLifecycleGate();
  // The sidebar's "what is it doing" word for every running conversation, page open or not.
  context.subscriptions.push(new ActivityWatcher(runtime, state, watchLifecycle));
  const diffDocuments = new DiffDocuments();
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(DiffDocuments.scheme, diffDocuments),
  );
  const conversation = new ConversationPanels(
    context.extensionUri,
    runtime,
    state,
    watchLifecycle,
    (binding, message) => {
      // Every action names the tab it came from. A draft tab has no session: its first message starts one,
      // and its chips pick what that start will use; a live tab's chips switch the running session.
      const session = binding.session;
      if (message.type === "prompt") {
        void run(() => afterReady(() => session
          ? controller.prompt(message.text, session, binding)
          : controller.sendDraft(binding, message.text)));
      } else if (message.type === "answerApproval") {
        if (!session) return;
        void run(() => afterReady(
          () => controller.answerApproval(message.approval, message.option, message.subjectDigest, session),
        ));
      } else if (message.type === "switchModel") {
        void run(() => afterReady(() => controller.switchModel(message.available, message.model, message.effort, binding)));
      } else if (message.type === "switchMode") {
        void run(() => afterReady(() => controller.switchMode(message.available, binding)));
      } else if (message.type === "switchEffort") {
        void run(() => afterReady(() => controller.switchEffort(message.model, binding)));
      } else if (message.type === "pickProject") {
        void run(() => afterReady(() => session
          ? controller.pickProjectForLive(binding)
          : controller.pickDraftProject(binding)));
      } else if (message.type === "pickService") {
        void run(() => afterReady(() => session
          ? controller.pickServiceForLive(binding)
          : controller.pickDraftService(binding)));
      } else if (message.type === "attach") {
        void run(() => afterReady(() => controller.attach(binding)));
      } else if (message.type === "pasteImage") {
        controller.addPastedAttachment(binding, message);
      } else if (message.type === "removeAttachment") {
        controller.removeAttachment(binding, message.index);
      } else if (message.type === "mentionFile") {
        void run(() => afterReady(() => controller.insertFileMention(session ?? undefined)));
      } else if (message.type === "openDiff") {
        // A change the service declared, opened where VS Code shows changes. No session needed: the
        // change's text came with the frame and goes straight to the editor.
        void run(() => diffDocuments.open(message.diff));
      } else if (message.type === "interrupt") {
        // Interrupt is dispatched by its own name, never as a fallback: an action this validator
        // accepts but no branch handles must do nothing, not stop a running agent.
        if (!session) return;
        void run(() => afterReady(() => controller.interrupt(session)));
      }
    },
    (session) => state.conversationOf(session.sessionId)?.title ?? sessionTitle(session),
    (session) => providerDisplayName(session.providerId, state.providers),
    (providerId) => providerId ? providerIcon(providerId, state.providers) : "sparkle",
    (session) => {
      // The focused tab is the selection: the tree highlight and every command that says "the current
      // conversation" follow whichever conversation tab the reader is actually in. A draft selects nothing.
      state.select(session?.sessionId ?? null);
    },
    async (session) => {
      // Where the conversation runs, for the chips: the folder's name and branch, or "No project" for the
      // scratch folder, whose path is an implementation detail nobody should read.
      const home = state.conversationOf(session.sessionId)?.homeWorkspace ?? session.workspace;
      const projectless = isProjectless(home, state.projectlessRoot);
      return {
        project: projectless ? NO_PROJECT_LABEL : workspaceName(home) || home,
        projectPath: projectless ? null : session.workspace,
        branch: projectless ? null : await readGitBranch(session.workspace),
      };
    },
  );
  conversation.rememberPlaces({
    read: (place) => {
      const value = context.workspaceState.get<unknown>(`runtrol.place.${place}`);
      return typeof value === "string" ? value : null;
    },
    write: (place, sessionId) => {
      void context.workspaceState.update(`runtrol.place.${place}`, sessionId ?? undefined);
    },
  });
  controller = new Controller(context, client, runtime, state, conversation, selection, projectStore);
  // The window's folders follow into the grant's roots. Enrollment read them once; without this, every folder
  // opened after first activation stayed outside conversation discovery, silently.
  const rootFollowing = new WorkspaceRootFollowing({
    client,
    integrationId: () => runtime.integrationId(),
    refreshRoots: () => controller.refreshAfterRootWidened(),
    openFolders: () => (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
    warn: (message) => void vscode.window.showWarningMessage(message),
  });
  const missionController = new MissionController(client, controller, state, context, diffDocuments);
  const candidateController = new CandidateController(client);
  const missions = new MissionTree(missionController);
  const conversations = new ConversationsTree(state, projectStore, agentTools, context.extensionUri);
  const usage = new UsageView(context.extensionUri, {
    usage: () => runtime.providersUsage(),
    providers: () => state.providers,
    now: () => Date.now(),
    fix: (provider) => afterReady(() => controller.fixService(provider)),
    signIn: (provider) => afterReady(async () => controller.signInProvider(provider)),
    discover: async () => {
      await vscode.commands.executeCommand("runtrol.discoverServices");
    },
    dispatch: (action) => void run(action),
  });

  context.subscriptions.push(
    state,
    controller,
    missionController,
    candidateController,
    agentTools,
    conversation,
    missions,
    conversations,
    usage,
    vscode.window.registerTreeDataProvider("runtrol.missions", missions),
    vscode.window.registerFileDecorationProvider(conversations.decorations),
    vscode.workspace.registerTextDocumentContentProvider("runtrol-mission", missionController.documentProvider()),
    vscode.commands.registerCommand(
      "runtrol.refresh",
      () => run(() => afterMissionReady(async () => Promise.all([
        controller.refreshChats(),
        missionController.refresh(),
      ]).then(() => undefined))),
    ),
    vscode.commands.registerCommand(
      "runtrol.restartExtensionHost",
      () => run(restartExtensionHost),
    ),
    vscode.commands.registerCommand(
      "runtrol.validateMission",
      () => run(() => afterMissionReady(() => missionController.validateMission())),
    ),
    vscode.commands.registerCommand(
      "runtrol.fanOutInstruction",
      () => run(() => afterMissionReady(() => missionController.fanOutInstruction())),
    ),
    vscode.commands.registerCommand(
      "runtrol.registerMissionGate",
      () => run(() => afterMissionReady(async () => {
        await missionController.registerGate();
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.openMission",
      (item) => run(() => afterMissionReady(() => missionController.openMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.startMission",
      (item) => run(() => afterMissionReady(() => missionController.startMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.scheduleMission",
      (item) => run(() => afterMissionReady(() => missionController.scheduleMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.cancelMissionSchedule",
      (item) => run(() => afterMissionReady(() => missionController.cancelMissionSchedule(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.continueMission",
      (item) => run(() => afterMissionReady(() => missionController.continueMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.armMissionAutoFlight",
      (item) => run(() => afterMissionReady(() => missionController.armMissionAutoFlight(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.disarmMissionAutoFlight",
      (item) => run(() => afterMissionReady(() => missionController.disarmMissionAutoFlight(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.continueReadyMissions",
      () => run(() => afterMissionReady(() => missionController.continueReadyMissions())),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewMissionLanding",
      (item) => run(() => afterMissionReady(() => missionController.reviewMissionLanding(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.launchFleet",
      (item) => run(() => afterMissionReady(() => missionController.launchFleet(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.prepareMissionTask",
      (item) => run(() => afterMissionReady(() => missionController.prepareTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.sendTaskInstruction",
      (item) => run(() => afterMissionReady(() => missionController.sendTaskInstruction(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.verifyMissionTask",
      (item) => run(() => afterMissionReady(() => missionController.verifyTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.retryMissionTask",
      (item) => run(() => afterMissionReady(() => missionController.retryTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.recoverInterruptedMission",
      (item) => run(() => afterMissionReady(() => missionController.recoverInterruptedMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.pauseMission",
      (item) => run(() => afterMissionReady(() => missionController.pauseMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.resumeMission",
      (item) => run(() => afterMissionReady(() => missionController.resumeMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.cancelMission",
      (item) => run(() => afterMissionReady(() => missionController.cancelMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.completeMissionIntegration",
      (item) => run(() => afterMissionReady(() => missionController.completeIntegration(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.compareMissionResults",
      (item) => run(() => afterMissionReady(() => missionController.compareResults(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.archiveMission",
      (item) => run(() => afterMissionReady(() => missionController.archiveMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openTaskSession",
      (item) => run(() => afterMissionReady(() => missionController.openTaskSession(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.proposeCapability",
      () => run(() => afterReady(() => candidateController.propose())),
    ),
    vscode.commands.registerCommand(
      "runtrol.capabilityInbox",
      () => run(() => afterReady(() => candidateController.inbox())),
    ),
    vscode.commands.registerCommand(
      "runtrol.verifyCapability",
      () => run(() => afterReady(() => candidateController.verify())),
    ),
    vscode.commands.registerCommand(
      "runtrol.approveCapability",
      () => run(() => afterReady(() => candidateController.approve())),
    ),
    vscode.commands.registerCommand(
      "runtrol.rejectCapability",
      () => run(() => afterReady(() => candidateController.reject())),
    ),
    vscode.commands.registerCommand(
      "runtrol.quarantineCapability",
      () => run(() => afterReady(() => candidateController.quarantine())),
    ),
    vscode.commands.registerCommand(
      "runtrol.rollbackCapability",
      () => run(() => afterReady(() => candidateController.rollback())),
    ),
    vscode.commands.registerCommand(
      "runtrol.archiveCapability",
      () => run(() => afterReady(() => candidateController.archive())),
    ),
    vscode.commands.registerCommand(
      "runtrol.checkProviderUpdates",
      () => run(() => afterReady(() => controller.checkProviderUpdates())),
    ),
    vscode.commands.registerCommand(
      "runtrol.remoteConnectionStatus",
      () => run(() => afterReady(async () => {
        const connection = await remoteConnection(client);
        await vscode.window.showInformationMessage(remoteConnectionMessage(connection));
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.pairPhone",
      () => run(() => afterReady(() => pairPhone(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewPhonePairings",
      () => run(() => afterReady(() => reviewPhonePairings(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.managePhones",
      () => run(() => afterReady(() => managePhones(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewIntegrations",
      () => run(() => afterReady(() => reviewIntegrationEnrollments(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.manageIntegrations",
      () => run(() => afterReady(async () => {
        if (await manageIntegrations(client)) await controller.reconnect();
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewRuntimeRequests",
      () => run(() => afterReady(() => reviewRuntimeRequests(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openNextWaiting",
      () => run(() => afterReady(() => controller.openNextWaiting())),
    ),
    vscode.commands.registerCommand(
      "runtrol.switchSession",
      () => run(() => afterReady(() => controller.switchSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.startSession",
      () => run(() => afterReady(() => controller.startSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.alsoAsk",
      () => run(() => afterReady(() => controller.alsoAskFocusedDraft())),
    ),
    vscode.commands.registerCommand(
      "runtrol.startConfiguredSession",
      () => run(() => afterReady(() => controller.startConfiguredSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.newConversationInProject",
      (item: unknown) => run(() => afterReady(async () => {
        // Inline on the project heading only, so the argument is always the heading. Guarded anyway, because a
        // command invoked with the wrong thing must refuse rather than start a session somewhere surprising.
        if (!(item instanceof ProjectItem)) return;
        await controller.startSessionInWorkspace(item.group.workspace);
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.newConfiguredConversationInProject",
      // The deliberate flow, one right-click away from the heading it starts in. The folder question is
      // already answered by the heading; service, model, effort, and mode are still asked.
      (item: unknown) => run(() => afterReady(async () => {
        if (!(item instanceof ProjectItem)) return;
        await controller.startConfiguredSessionInWorkspace(item.group.workspace);
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.createProject",
      // No name prompt: the folder's own name is the default and rename is one right-click away. Several
      // folders can be picked at once, and each becomes its own project.
      () => run(async () => {
        const chosen = await vscode.window.showOpenDialog({
          canSelectFiles: false,
          canSelectFolders: true,
          canSelectMany: true,
          openLabel: "Create Project",
          title: "Choose the folder each new project stands on",
          defaultUri: vscode.workspace.workspaceFolders?.[0]?.uri,
        });
        if (!chosen) return;
        for (const folder of chosen) {
          await projectStore.create(folder.fsPath);
        }
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.createProjectHere",
      // A discovered or open folder promoted to a created project, in one click from its heading. The folder
      // is already answered by the heading, so no dialog; rename is one right-click away afterwards.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.create(item.group.workspace);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.enableAgentTools",
      (item?: unknown) => run(() => afterReady(
        () => changeAgentTools(agentTools, "enable", item instanceof ProjectItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.disableAgentTools",
      (item?: unknown) => run(() => afterReady(
        () => changeAgentTools(agentTools, "disable", item instanceof ProjectItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.renameProject",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        const name = await vscode.window.showInputBox({
          prompt: "Project name",
          value: item.group.name,
        });
        if (name === undefined) return;
        await projectStore.setName(item.group.workspace, name);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.removeProject",
      // Removal only takes the heading away: conversations stay, and creating the project again is one click.
      // That reversibility is why there is no confirmation dialog in the way; the toast's Undo covers the
      // misclick without making everyone else answer a question first.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        const { workspace, name } = item.group;
        await projectStore.remove(workspace);
        // Not awaited as a gate: the removal is done, and the toast lives on its own time.
        void vscode.window.showInformationMessage(`Removed the project ${name}.`, "Undo").then((choice) => {
          if (choice === "Undo") {
            return run(() => projectStore.create(workspace, name).then(() => undefined));
          }
          return undefined;
        });
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.fixService",
      // From the Command Palette. The fixed usage surface routes its already visible service directly.
      () => run(async () => {
        await afterReady(async () => {
          const broken = state.providers.filter(isBroken);
          if (broken.length === 0) {
            void vscode.window.showInformationMessage("All installed coding services are available.");
            return;
          }
          let provider = broken.at(0) ?? null;
          if (broken.length > 1) {
            const picked = await vscode.window.showQuickPick(
              broken.map((candidate) => ({
                label: candidate.displayName,
                description: "Unavailable",
                detail: candidate.installation.why ?? undefined,
                provider: candidate,
              })),
              {
                title: "Fix coding service",
                placeHolder: "Choose the service that needs attention",
              },
            );
            provider = picked?.provider ?? null;
          }
          if (provider) await controller.fixService(provider);
        });
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.discoverServices",
      () => run(() => afterReady(async () => {
        const installable = installableProviders(state.providers);
        if (installable.length === 0) {
          void vscode.window.showInformationMessage("Every catalogued coding service is already installed or needs a manual installer.");
          return;
        }
        const picked = await vscode.window.showQuickPick(
          installable.map((provider) => ({
            label: provider.displayName,
            description: "Not installed",
            detail: provider.help?.install ?? undefined,
            provider,
          })),
          {
            title: "Add coding service",
            placeHolder: "Choose a service. Its command is placed in the terminal and never run automatically.",
          },
        );
        if (picked) await controller.fixService(picked.provider);
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.selectSession",
      (item) => run(() => afterReady(() => controller.select(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openConversation",
      () => run(() => afterReady(() => controller.openConversation())),
    ),
    vscode.commands.registerCommand(
      "runtrol.renameSession",
      (item) => run(() => afterReady(() => controller.nameSession(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openWorkspace",
      (item) => run(() => afterReady(() => controller.openWorkspace(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openProjectWorkspace",
      (item) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        // The explicit move the contract requires: only this button changes what the window is open on.
        await controller.switchWindowTo(item.group.workspace);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.returnToPreviousProject",
      () => run(() => controller.returnToPreviousProject()),
    ),
    vscode.commands.registerCommand(
      "runtrol.switchProject",
      () => run(() => afterReady(() => controller.switchProject())),
    ),
    vscode.commands.registerCommand(
      "runtrol.signInFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.signInFromRow(item instanceof ConversationItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.allowFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "allow"),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.declineFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "decline"),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.answerFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "choose"),
      )),
    ),
    vscode.commands.registerCommand("runtrol.interrupt", () => run(() => afterReady(() => controller.interrupt()))),
    vscode.commands.registerCommand(
      "runtrol.closeSession",
      (item) => run(() => afterReady(() => controller.close(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.explainListing",
      () => {
        const reasons = conversations.listingReasons();
        void vscode.window.showInformationMessage(
          reasons
            ? `Not every chat is listed. ${reasons}`
            : "Every conversation the installed coding services list is shown.",
        );
      },
    ),
    vscode.commands.registerCommand(
      "runtrol.openConversationInPanel",
      (item?: unknown) => run(() => afterReady(
        () => controller.placeConversation("panel", item instanceof ConversationItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.openConversationInSideBar",
      (item?: unknown) => run(() => afterReady(
        () => controller.placeConversation("sideBar", item instanceof ConversationItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.openConversationInTab",
      (item?: unknown) => run(() => afterReady(
        () => controller.placeConversation("tab", item instanceof ConversationItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.arrangeConversationGrid",
      () => run(() => afterReady(() => controller.arrangeConversationGrid())),
    ),
    // The two workbench places a conversation can live in besides a tab. Resolved by VS Code when first
    // shown; a conversation placed there before a reload comes back once the session list is ready.
    vscode.window.registerWebviewViewProvider(
      PANEL_VIEW_ID,
      conversation.viewProvider("panel", (place, sessionId) => {
        void run(() => afterReady(async () => {
          const session = state.sessions.find((candidate) => candidate.sessionId === sessionId) ?? null;
          if (session) await controller.placeConversation(place, session);
        }));
      }),
      { webviewOptions: { retainContextWhenHidden: false } },
    ),
    vscode.window.registerWebviewViewProvider(
      SIDE_BAR_VIEW_ID,
      conversation.viewProvider("sideBar", (place, sessionId) => {
        void run(() => afterReady(async () => {
          const session = state.sessions.find((candidate) => candidate.sessionId === sessionId) ?? null;
          if (session) await controller.placeConversation(place, session);
        }));
      }),
      { webviewOptions: { retainContextWhenHidden: false } },
    ),
    vscode.commands.registerCommand(
      "runtrol.archiveConversation",
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.archiveConversation(item));
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.deleteConversation",
      // From the row's X. Guarded, because a command invoked with the wrong thing must refuse rather than
      // delete something surprising.
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.deleteConversation(item));
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.pinConversation",
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.togglePin(item));
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.unpinConversation",
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.togglePin(item));
      }),
    ),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("runtrol.corePath")) {
        const previous = lifecycle;
        lifecycle = previous.catch(() => undefined).then(async () => {
          locator.invalidate();
          await controller.reconnect();
        });
        void run(async () => {
          await lifecycle;
          await configureRemoteConnection(client);
        });
      } else if (event.affectsConfiguration("runtrol.relayOrigin")) {
        void run(() => afterReady(async () => {
          await configureRemoteConnection(client);
        }));
      }
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      void run(() => afterReady(() => rootFollowing.follow()));
    }),
  );
  // Before the Runtime integration speaks: proving the daemon that answered is the installed generation
  // is what lets everything past the hello assume the daemon and this extension are the same build.
  const runtimeInitialization = superviseCoreCurrency(client, locator).then(() => runtime.initialize());
  const controllerInitialization = runtimeInitialization.then(async () => {
    initializationStage = "controller";
    await controller.initialize();
  });
  missionLifecycle = missionController.initialize().catch((error: unknown) => {
    initializationStage = "mission";
    throw error;
  });
  const readyInitialization = Promise.all([controllerInitialization, missionLifecycle]).then(() => {
    missionController.startAutoFlights();
  });
  readyInitialization.then(
    () => {
      initializationStage = "ready";
      settleReady?.();
      // After ready rather than at enrollment, so a window opened onto a not-yet-approved folder catches up on
      // its own activation, which is the same physical act the first enrollment trusted.
      void run(() => rootFollowing.follow());
    },
    (error: unknown) => {
      settleReady?.(error);
    },
  );
  context.subscriptions.push(
    vscode.window.registerWebviewPanelSerializer(ConversationView.viewType, {
      deserializeWebviewPanel: async (panel, webviewState: unknown) => {
        // The restored tab names its own session, or the draft it was showing (stamped into webview state on
        // every reset). Rebinding waits for the list; a tab whose session no longer exists closes rather than
        // showing a guess, and a draft comes back with the choices it had.
        const stamped = (webviewState ?? {}) as { sessionId?: unknown; draft?: unknown };
        const draft = readDraftState(stamped.draft);
        await afterReady(async () => {
          if (draft) {
            await controller.restoreDraft(panel, draft);
            return;
          }
          const session = typeof stamped.sessionId === "string"
            ? state.sessions.find((candidate) => candidate.sessionId === stamped.sessionId) ?? null
            : null;
          if (!session) {
            panel.dispose();
            return;
          }
          await conversation.adopt(panel, session);
        });
      },
    }),
  );
  const conversationsView = vscode.window.createTreeView("runtrol.sessions", {
    treeDataProvider: conversations,
  });
  conversations.bindView(conversationsView);
  const usageRegistration = vscode.window.registerWebviewViewProvider("runtrol.usage", usage, {
    webviewOptions: { retainContextWhenHidden: false },
  });
  let revealingUsage = false;
  const ensureUsageVisible = async (): Promise<void> => {
    if (usage.visible || revealingUsage || !conversationsView.visible) return;
    revealingUsage = true;
    try {
      await vscode.commands.executeCommand("runtrol.usage.focus");
      await vscode.commands.executeCommand("runtrol.sessions.focus");
    } finally {
      revealingUsage = false;
    }
  };
  context.subscriptions.push(
    usageRegistration,
    conversationsView.onDidChangeVisibility((event) => {
      if (event.visible) void ensureUsageVisible();
    }),
    state.onDidChange((change) => {
      if (change === "rows") usage.sessionsChanged();
      if (change === "usage") usage.usageChanged(state.usage);
    }),
  );
  if (conversationsView.visible) void ensureUsageVisible();
  const revealEntryConversation = (): void => {
    void run(() => afterReady(() => controller.revealConversationOnEntry()));
  };
  if (conversationsView.visible) {
    revealEntryConversation();
  }
  context.subscriptions.push(
    conversationsView,
    conversationsView.onDidChangeVisibility((event) => {
      if (event.visible) revealEntryConversation();
    }),
  );
  void run(() => lifecycle);
  void run(() => missionLifecycle);
  void run(async () => {
    await lifecycle;
    await configureRemoteConnection(client);
  });
  void run(async () => {
    await lifecycle;
    const workingDirectory = vscode.workspace.workspaceFolders
      ?.find((folder) => folder.uri.scheme === "file")
      ?.uri.fsPath ?? vscode.env.appRoot;
    await agentTools.refresh(workingDirectory);
  });
  return {
    get ready() {
      return lifecycle;
    },
    get initializationStage() {
      return MEASURED_HOST ? initializationStage : undefined;
    },
    refresh: () => afterReady(() => controller.refresh()),
    measureWebview: MEASURED_HOST
      ? (framesPerSecond, durationMs) => afterReady(async () => {
        const focused = conversation.focused();
        if (!focused) throw new Error("no conversation tab is open to measure");
        return focused.view.measurePerformance(framesPerSecond, durationMs);
      })
      : undefined,
    measureSessionManagement: MEASURED_HOST
      ? (sessionIds, progress = () => {}) => afterReady(async () => {
        const expected = new Set(sessionIds);
        const managed = state.sessions.filter((session) => expected.has(session.sessionId));
        const initialHot = managed.filter((session) => session.hot);
        const cold = managed.find((session) => !session.hot);
        if (expected.size !== 30 || managed.length !== expected.size || initialHot.length !== 8 || !cold) {
          throw new Error(
            `expected 30 named sessions with eight hot and a cold choice, found ${managed.length} and ${initialHot.length}`,
          );
        }

        const resumeStarted = performance.now();
        progress("cold-select");
        // The extension-host integration owns the whole measurement hang guard. Per-phase timers here would
        // abort a valid trial on a saturated runner before the three-trial performance ratchet can score it.
        await controller.select(cold.sessionId);
        progress("cold-watch-and-render");
        await Promise.all([
          controller.selectedWatchReady(),
          conversation.focused()?.settled() ?? Promise.resolve(),
        ]);
        const coldResumeMs = performance.now() - resumeStarted;
        const resumed = state.selected;
        if (
          !resumed
          || !resumed.hot
          || resumed.sessionId !== cold.sessionId
          || resumed.providerId !== cold.providerId
          || resumed.nativeSessionId !== cold.nativeSessionId
          || resumed.workspace !== cold.workspace
        ) {
          throw new Error("selecting a cold row did not heat the same Runtime-managed session");
        }
        const current = state.sessions.filter((session) => expected.has(session.sessionId));
        const hot = current.filter((session) => session.hot);
        if (current.length !== 30 || hot.length !== 8) {
          throw new Error(`cold resume changed the 30-session and eight-hot bounds to ${current.length} and ${hot.length}`);
        }
        const samples: number[] = [];
        for (let round = 0; round < SESSION_SWITCH_ROUNDS; round += 1) {
          for (const [index, session] of hot.entries()) {
            const started = performance.now();
            progress(`round-${round + 1}-session-${index + 1}-select`);
            await controller.select(session.sessionId);
            progress(`round-${round + 1}-session-${index + 1}-watch`);
            await controller.selectedWatchReady();
            samples.push(performance.now() - started);
          }
        }
        progress("selection-persistence");
        await controller.selectionPersisted();
        return {
          sessionCount: current.length,
          hotSessionCount: hot.length,
          coldResumeMs,
          sessionSwitchP95Ms: percentile(samples, 0.95),
          resumedFrom: cold.sessionId,
          resumedTo: resumed.sessionId,
          restoreSession: state.selected?.sessionId ?? "",
          restoreWorkspace: state.selected?.workspace ?? "",
        };
      })
      : undefined,
    verifyRestoredSession: MEASURED_HOST
      ? (sessionId) => afterReady(async () => {
        if (state.selected?.sessionId !== sessionId) {
          throw new Error(`restored ${state.selected?.sessionId ?? "no session"}, expected ${sessionId}`);
        }
        await within(controller.selectedWatchReady(), 10_000, "selected-session watch handshake");
      })
      : undefined,
    // The two follow probes exist for the live root-following proof: a real window opens a second folder and the
    // harness watches that folder's provider-owned stored conversation arrive. Managed sessions are deliberately
    // machine-wide on this owner-only local surface, so they cannot prove that a discovery root widened.
    hasConversationIn: MEASURED_HOST
      ? (folder) => afterReady(async () => nativeConversationVisibleIn(state, folder))
      : undefined,
    waitForConversationIn: MEASURED_HOST
      ? (folder, deadlineMs) => afterReady(() => new Promise<number>((resolve, reject) => {
        const arrived = () => nativeConversationVisibleIn(state, folder);
        if (arrived()) {
          resolve(0);
          return;
        }
        const started = performance.now();
        const timer = setTimeout(() => {
          subscription.dispose();
          reject(new Error(`no conversation arrived for ${folder} within ${deadlineMs} ms`));
        }, deadlineMs);
        const subscription = state.onDidChange(() => {
          if (!arrived()) return;
          clearTimeout(timer);
          subscription.dispose();
          resolve(performance.now() - started);
        });
      }))
      : undefined,
    // The harness's way to stand a created project up without driving the folder-picker dialog. Same code path
    // as the command, minus the picking.
    seedProject: MEASURED_HOST || RUNTROL_INCLUDE_TEST_JOURNEY
      ? async (folder) => {
        await projectStore.create(folder);
      }
      : undefined,
    openFirstConversation: MEASURED_HOST
      ? async () => {
        // The eye pass photographs a real conversation, so it opens the first one the tree would show,
        // through the same selection path a click takes.
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null, state.projectlessRoot);
        const openable = rows.find((row) => row.canOpen);
        if (!openable) {
          throw new Error("no openable conversation for the eye pass");
        }
        await controller.select(openable);
        await vscode.commands.executeCommand("runtrol.openConversation");
      }
      : undefined,
    openCrossProjectConversation: MEASURED_HOST
      ? async () => {
        // The contract in `docs/vscodeSurface.md`, provable: a conversation whose folder this window never
        // opened selects and opens as a tab right here. Managed rows only, because native rows still ride
        // the enrollment roots until discovery goes machine-wide.
        const open = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null, state.projectlessRoot);
        const away = rows.find((row) =>
          row.canOpen
          && row.session !== null
          && !open.some((folder) => workspaceCovers(folder, row.workspace)));
        if (!away) {
          throw new Error("no conversation outside the open folders to prove the contract with");
        }
        await controller.select(away);
        await vscode.commands.executeCommand("runtrol.openConversation");
        if (state.selected?.sessionId !== away.session?.sessionId) {
          throw new Error("selecting the away conversation did not take");
        }
      }
      : undefined,
    journey: RUNTROL_INCLUDE_TEST_JOURNEY
      ? journeyApi(
        controller,
        missionController,
        state,
        conversation,
        afterReady,
        context.extensionMode,
        (sessionId) => conversations.revealSession(sessionId),
        (key) => conversations.revealConversation(key),
        () => conversations.treeItemIdsForJourney(),
      )
      : undefined,
  };
}

/// Whether provider discovery has made a stored conversation in this folder visible.
function nativeConversationVisibleIn(state: RuntimeState, folder: string): boolean {
  const identity = workspaceIdentity(folder);
  return state.nativeChats.some((chat) => workspaceIdentity(chat.cwd) === identity);
}

function testIntegrationRoots(context: vscode.ExtensionContext): readonly string[] {
  if (context.extensionMode !== vscode.ExtensionMode.Test) return [];
  const raw = process.env.RUNTROL_TEST_INTEGRATION_ROOTS;
  if (!raw) return [];
  const value: unknown = JSON.parse(raw);
  if (
    !Array.isArray(value)
    || value.length > 32
    || !value.every((root) => typeof root === "string" && path.isAbsolute(root))
  ) {
    throw new Error("RUNTROL_TEST_INTEGRATION_ROOTS must contain at most 32 absolute paths");
  }
  return [...new Set(value)];
}

async function changeAgentTools(
  controller: AgentToolsController,
  action: AgentToolsAction,
  item?: ProjectItem,
): Promise<void> {
  const workspace = item?.group.workspace ?? await chooseAgentToolsProject();
  if (!workspace) return;
  const name = path.basename(workspace) || workspace;
  const changing = action === "enable" ? "Enabling" : "Disabling";
  const result = await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `${changing} Agent Tools for ${name}`,
      cancellable: false,
    },
    () => action === "enable" ? controller.enable(workspace) : controller.disable(workspace),
  );
  const warning = result.lines.find((line) => line.startsWith("warning:"));
  if (warning) void vscode.window.showWarningMessage(warning);
  if (action === "enable") {
    void vscode.window.showInformationMessage(
      `Agent Tools are ready for ${name}. Coding agents can now delegate through Runtrol; approvals stay with you.`,
    );
  } else {
    void vscode.window.showInformationMessage(
      result.alreadySettled
        ? `Agent Tools were already off for ${name}.`
        : `Agent Tools are off for ${name}. Runtime authority and local credentials were removed.`,
    );
  }
}

async function chooseAgentToolsProject(): Promise<string | null> {
  const folders = (vscode.workspace.workspaceFolders ?? [])
    .filter((folder) => folder.uri.scheme === "file");
  if (folders.length === 0) {
    await vscode.window.showWarningMessage("Open a local project folder before enabling Agent Tools.");
    return null;
  }
  if (folders.length === 1) return folders[0]?.uri.fsPath ?? null;
  const picked = await vscode.window.showQuickPick(
    folders.map((folder) => ({
      label: folder.name,
      detail: folder.uri.fsPath,
      workspace: folder.uri.fsPath,
    })),
    {
      title: "Project for Agent Tools",
      placeHolder: "Choose the one project root coding agents may orchestrate",
      matchOnDetail: true,
    },
  );
  return picked?.workspace ?? null;
}

export function deactivate(): void {}

async function configureRemoteConnection(client: CoreClient): Promise<RemoteConnection> {
  const configured = vscode.workspace
    .getConfiguration("runtrol")
    .get<string>("relayOrigin", "")
    .trim();
  const { response } = await client.once({
    ask: "remoteConfigure",
    with: { relay_origin: configured || null },
  });
  return readRemoteConnection(response);
}

async function remoteConnection(client: CoreClient): Promise<RemoteConnection> {
  const { response } = await client.once({ ask: "remoteConnection" });
  return readRemoteConnection(response);
}

function readRemoteConnection(response: Awaited<ReturnType<CoreClient["once"]>>["response"]): RemoteConnection {
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  if (response.say !== "remoteConnection") {
    throw new Error(`the Core answered remote connection status with ${response.say}`);
  }
  return response.with;
}

function remoteConnectionMessage(connection: RemoteConnection): string {
  if (connection.state === "disabled") {
    return "Runtrol phone connection is disabled. Set runtrol.relayOrigin to enable it.";
  }
  if (connection.state === "online") {
    return `Runtrol phone connection is online through ${connection.relay_origin ?? "the configured relay"}.`;
  }
  if (connection.state === "connecting") {
    return `Runtrol phone connection is connecting to ${connection.relay_origin ?? "the configured relay"}.`;
  }
  return `Runtrol phone connection is retrying after ${connection.stage ?? "relay"} failure.`;
}

async function run(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    // Already explained, with the coding service's own next steps offered as buttons. A second message
    // underneath that one reads as a second problem, and the bare protocol string is the less useful of
    // the two.
    if (error instanceof ServiceTroubleReported) return;
    await vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error));
  }
}

async function restartExtensionHost(): Promise<void> {
  const confirmed = await vscode.window.showWarningMessage(
    "Restart the VS Code Extension Host? Other extensions in this window will restart too.",
    { modal: true },
    "Restart extensions",
  );
  if (confirmed !== "Restart extensions") return;
  await vscode.commands.executeCommand("workbench.action.restartExtensionHost");
}

function percentile(values: readonly number[], at: number): number {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * at) - 1] ?? Number.POSITIVE_INFINITY;
}

function within<T>(work: Promise<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) {
      clearTimeout(timer);
    }
  });
}
