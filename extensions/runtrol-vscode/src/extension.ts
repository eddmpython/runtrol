import path from "node:path";

import * as vscode from "vscode";

import { CandidateController } from "./capability/controller";
import { conversations as conversationRows } from "./conversationList";
import { ConversationPanels } from "./conversationPanels";
import { ConversationView, type WebviewPerformance } from "./conversationView";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import {
  confirmRuntimeForget,
  manageIntegrations,
  reviewIntegrationEnrollments,
  reviewRuntimeRequests,
  selfApproveIntegration,
} from "./integrationAdministration";
import { journeyApi, type JourneyApi } from "./journeyApi";
import { MissionController } from "./mission/controller";
import { MissionTree } from "./mission/tree";
import { ProjectStore } from "./projects";
import { managePhones, pairPhone, reviewPhonePairings } from "./pairingAdministration";
import type { RemoteConnection } from "./protocol";
import { SelectionStore } from "./selectionStore";
import { ServiceTroubleReported } from "./serviceHelp";
import { providerDisplayName, sessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { workspaceCovers, workspaceIdentity } from "./workspaceCollision";
import { WorkspaceRootFollowing } from "./workspaceRoots";
import { ConversationsTree, ProjectItem, ServiceProblemItem } from "./trees";
import { UsageTree } from "./usageTree";

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<WebviewPerformance>;
  measureSessionManagement?(sessionIds: readonly string[]): Promise<SessionManagementPerformance>;
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
  let initializationStage = "runtime:bootstrap";
  const runtime = new StudioRuntimeClient(
    context,
    () => locator.runtimeExecutable(),
    (pendingId, signature) => selfApproveIntegration(client, pendingId, signature),
    (confirmationId, sessionId) => confirmRuntimeForget(client, confirmationId, sessionId),
    testIntegrationRoots(context),
    (stage) => {
      initializationStage = `runtime:${stage}`;
    },
  );
  const state = new RuntimeState();
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
  const conversation = new ConversationPanels(
    context.extensionUri,
    runtime,
    state,
    (session, message) => {
      if (message.type === "prompt") {
        void run(() => afterReady(() => controller.prompt(message.text, session)));
      } else if (message.type === "startChat") {
        void run(() => afterReady(() => controller.startSession()));
      } else if (message.type === "startConfiguredChat") {
        void run(() => afterReady(() => controller.startConfiguredSession()));
      } else if (message.type === "answerApproval") {
        void run(() => afterReady(
          () => controller.answerApproval(message.approval, message.option, message.subjectDigest, session),
        ));
      } else if (message.type === "switchModel") {
        void run(() => afterReady(() => controller.switchModel(message.available)));
      } else if (message.type === "switchMode") {
        void run(() => afterReady(() => controller.switchMode(message.available)));
      } else if (message.type === "switchEffort") {
        void run(() => afterReady(() => controller.switchEffort(message.model)));
      } else if (message.type === "mentionFile") {
        void run(() => afterReady(() => controller.insertFileMention(session)));
      } else if (message.type === "interrupt") {
        // Interrupt is dispatched by its own name, never as a fallback: an action this validator
        // accepts but no branch handles must do nothing, not stop a running agent.
        void run(() => afterReady(() => controller.interrupt(session)));
      }
    },
    (session) => state.conversationOf(session.sessionId)?.title ?? sessionTitle(session),
    (session) => providerDisplayName(session.providerId, state.providers),
    (session) => {
      // The focused tab is the selection: the tree highlight and every command that says "the current
      // conversation" follow whichever conversation tab the reader is actually in.
      state.select(session.sessionId);
    },
  );
  controller = new Controller(context, client, runtime, state, conversation, selection);
  // The window's folders follow into the grant's roots. Enrollment read them once; without this, every folder
  // opened after first activation stayed outside conversation discovery, silently.
  const rootFollowing = new WorkspaceRootFollowing({
    client,
    integrationId: () => runtime.integrationId(),
    refreshRoots: () => controller.refreshAfterRootWidened(),
    openFolders: () => (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
    warn: (message) => void vscode.window.showWarningMessage(message),
  });
  const missionController = new MissionController(client, controller, state, context);
  const candidateController = new CandidateController(client);
  const missions = new MissionTree(missionController);
  // The operator's own projects. A heading exists because they created it here, never because a folder
  // happened to hold conversations. Global state, because the panel manages the whole machine from any window.
  const projectStore = new ProjectStore(context.globalState);
  const conversations = new ConversationsTree(state, projectStore);
  const usage = new UsageTree({
    usage: () => runtime.providersUsage(),
    providers: () => state.providers,
    now: () => Date.now(),
  });

  context.subscriptions.push(
    state,
    controller,
    missionController,
    candidateController,
    conversation,
    missions,
    conversations,
    vscode.window.registerTreeDataProvider("runtrol.missions", missions),
    vscode.window.registerFileDecorationProvider(conversations.decorations),
    usage,
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
      // From the problem row, so nobody has to attempt a conversation just to be told the remedy.
      (item: unknown) => run(async () => {
        if (!(item instanceof ServiceProblemItem)) return;
        await afterReady(() => controller.fixService(item.provider));
      }),
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
        await vscode.commands.executeCommand(
          "vscode.openFolder",
          vscode.Uri.file(item.group.workspace),
          { forceNewWindow: false },
        );
      }),
    ),
    vscode.commands.registerCommand("runtrol.interrupt", () => run(() => afterReady(() => controller.interrupt()))),
    vscode.commands.registerCommand(
      "runtrol.closeSession",
      (item) => run(() => afterReady(() => controller.close(item))),
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
  const runtimeInitialization = runtime.initialize();
  const controllerInitialization = runtimeInitialization.then(async () => {
    initializationStage = "controller";
    await controller.initialize();
  });
  missionLifecycle = missionController.initialize().catch((error: unknown) => {
    initializationStage = "mission";
    throw error;
  });
  controllerInitialization.then(
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
        // The restored tab names its own session (stamped into webview state on every reset). Rebinding
        // waits for the list; a tab whose session no longer exists closes rather than showing a guess.
        const named = (webviewState as { sessionId?: unknown } | undefined)?.sessionId;
        await afterReady(async () => {
          const session = typeof named === "string"
            ? state.sessions.find((candidate) => candidate.sessionId === named) ?? null
            : state.selected;
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
  const usageView = vscode.window.createTreeView("runtrol.usage", {
    treeDataProvider: usage,
  });
  usage.bindView(usageView);
  context.subscriptions.push(
    usageView,
    // The strip follows the session list's own changes, which is when a gauge is most likely to have moved.
    state.onDidChange((change) => {
      if (change === "rows") usage.sessionsChanged();
    }),
  );
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
      ? (sessionIds) => afterReady(async () => {
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
        await performanceDeadline(
          controller.select(cold.sessionId),
          10_000,
          "cold session selection",
        );
        await Promise.all([
          performanceDeadline(
            controller.selectedWatchReady(),
            10_000,
            "cold session event watch",
          ),
          performanceDeadline(
            conversation.focused()?.settled() ?? Promise.resolve(),
            10_000,
            "cold session Webview render",
          ),
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
          for (const session of hot) {
            const started = performance.now();
            await controller.select(session.sessionId);
            await controller.selectedWatchReady();
            samples.push(performance.now() - started);
          }
        }
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
    // harness watches that folder's conversation arrive. They consult both collections the conversation tree
    // merges (supervised sessions and the provider-owned stored chats), through the same identity function
    // collision detection uses, so the probe agrees with the product about what "a conversation here" means.
    hasConversationIn: MEASURED_HOST
      ? (folder) => afterReady(async () => conversationVisibleIn(state, folder))
      : undefined,
    waitForConversationIn: MEASURED_HOST
      ? (folder, deadlineMs) => afterReady(() => new Promise<number>((resolve, reject) => {
        const arrived = () => conversationVisibleIn(state, folder);
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
    seedProject: MEASURED_HOST
      ? async (folder) => {
        await projectStore.create(folder);
      }
      : undefined,
    openFirstConversation: MEASURED_HOST
      ? async () => {
        // The eye pass photographs a real conversation, so it opens the first one the tree would show,
        // through the same selection path a click takes.
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null);
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
        // The contract in memory/uxContract.md, provable: a conversation whose folder this window never
        // opened selects and opens as a tab right here. Managed rows only, because native rows still ride
        // the enrollment roots until discovery goes machine-wide.
        const open = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null);
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
    journey: journeyApi(controller, state, conversation, afterReady, context.extensionMode),
  };
}

/// Whether any conversation, supervised or provider-owned, lives in this folder. The follow probes' one lens.
function conversationVisibleIn(state: RuntimeState, folder: string): boolean {
  const identity = workspaceIdentity(folder);
  return state.sessions.some((session) => workspaceIdentity(session.workspace) === identity)
    || state.nativeChats.some((chat) => workspaceIdentity(chat.cwd) === identity);
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

function performanceDeadline<T>(pending: Promise<T>, timeoutMs: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error(`${label} exceeded ${timeoutMs} ms`)),
      timeoutMs,
    );
    pending.then(
      (value) => {
        clearTimeout(timeout);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timeout);
        reject(error);
      },
    );
  });
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
    await vscode.window.showErrorMessage(`Runtrol: ${error instanceof Error ? error.message : String(error)}`);
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
