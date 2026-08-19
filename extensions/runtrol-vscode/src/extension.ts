import path from "node:path";

import * as vscode from "vscode";

import { CandidateController } from "./capability/controller";
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
import { managePhones, pairPhone, reviewPhonePairings } from "./pairingAdministration";
import type { RemoteConnection } from "./protocol";
import { SelectionStore } from "./selectionStore";
import { ServiceTroubleReported } from "./serviceHelp";
import { providerDisplayName, sessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { WorkspaceRootFollowing } from "./workspaceRoots";
import { ConversationsTree, ProjectItem } from "./trees";

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<WebviewPerformance>;
  measureSessionManagement?(sessionIds: readonly string[]): Promise<SessionManagementPerformance>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
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
  const conversation = new ConversationView(
    context.extensionUri,
    (message) => {
      if (message.type === "prompt") {
        void run(() => afterReady(() => controller.prompt(message.text)));
      } else if (message.type === "startChat") {
        void run(() => afterReady(() => controller.startSession()));
      } else if (message.type === "answerApproval") {
        void run(() => afterReady(
          () => controller.answerApproval(message.approval, message.option, message.subjectDigest),
        ));
      } else {
        void run(() => afterReady(() => controller.interrupt()));
      }
    },
    (session) => state.conversationOf(session.sessionId)?.title ?? sessionTitle(session),
    (visible) => controller.conversationVisibilityChanged(visible),
    (session) => providerDisplayName(session.providerId, state.providers),
  );
  controller = new Controller(context, client, runtime, state, conversation, selection);
  // The window's folders follow into the grant's roots. Enrollment read them once; without this, every folder
  // opened after first activation stayed outside conversation discovery, silently.
  const rootFollowing = new WorkspaceRootFollowing({
    client,
    integrationId: () => runtime.integrationId(),
    reconnect: () => controller.reconnect(),
    openFolders: () => (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
    warn: (message) => void vscode.window.showWarningMessage(message),
  });
  const missionController = new MissionController(client, controller, state);
  const candidateController = new CandidateController(client);
  const missions = new MissionTree(missionController);
  const conversations = new ConversationsTree(state);

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
      () => run(() => afterMissionReady(() => missionController.registerGate())),
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
      deserializeWebviewPanel: async (panel) => {
        await conversation.adopt(panel);
        await afterReady(async () => {
          conversation.reset(state.selected);
        });
      },
    }),
  );
  const conversationsView = vscode.window.createTreeView("runtrol.sessions", {
    treeDataProvider: conversations,
  });
  conversations.bindView(conversationsView);
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
      return process.env.RUNTROL_VSCODE_PERFORMANCE === "1" ? initializationStage : undefined;
    },
    refresh: () => afterReady(() => controller.refresh()),
    measureWebview: process.env.RUNTROL_VSCODE_PERFORMANCE === "1"
      ? (framesPerSecond, durationMs) => afterReady(
        () => conversation.measurePerformance(framesPerSecond, durationMs),
      )
      : undefined,
    measureSessionManagement: process.env.RUNTROL_VSCODE_PERFORMANCE === "1"
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
          controller.select(cold.sessionId, false),
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
            conversation.waitForCurrentRender(),
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
            await controller.select(session.sessionId, false);
            await Promise.all([
              controller.selectedWatchReady(),
              conversation.waitForCurrentRender(),
            ]);
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
    verifyRestoredSession: process.env.RUNTROL_VSCODE_PERFORMANCE === "1"
      ? (sessionId) => afterReady(async () => {
        if (state.selected?.sessionId !== sessionId) {
          throw new Error(`restored ${state.selected?.sessionId ?? "no session"}, expected ${sessionId}`);
        }
        await Promise.all([
          within(controller.selectedWatchReady(), 10_000, "selected-session watch handshake"),
          within(conversation.waitForCurrentRender(), 10_000, "selected-session Webview render"),
        ]);
      })
      : undefined,
    journey: journeyApi(controller, state, conversation, afterReady, context.extensionMode),
  };
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
