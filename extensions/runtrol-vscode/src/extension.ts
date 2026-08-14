import { readFile } from "node:fs/promises";
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
  reviewIntegrationEnrollment,
  reviewIntegrationEnrollments,
  reviewRuntimeRequests,
} from "./integrationAdministration";
import { journeyApi, type JourneyApi } from "./journeyApi";
import { MissionController } from "./mission/controller";
import { MissionTree } from "./mission/tree";
import { managePhones, pairPhone, reviewPhonePairings } from "./pairingAdministration";
import type { RemoteConnection } from "./protocol";
import { SelectionStore } from "./selectionStore";
import { uniqueSessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { ProvidersTree, SessionsTree } from "./trees";

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

export function activate(context: vscode.ExtensionContext): RuntrolExtensionApi {
  const locator = new CoreLocator(context);
  const client = new CoreClient(locator);
  let initializationStage = "runtime:bootstrap";
  // Extension Host gates delegate the physical-presence step to a separate local IPC driver. Installed-package
  // continuity explicitly opts in because VS Code reports the installed bundle as Production. The driver still
  // completes the real bounded challenge, and every ordinary activation shows the local review UI.
  const externalTestApproval = (
    context.extensionMode === vscode.ExtensionMode.Test
    || process.env.RUNTROL_TEST_INSTALLED_UPGRADE === "1"
  )
    ? process.env.RUNTROL_TEST_EXTERNAL_INTEGRATION_APPROVAL
    : undefined;
  const runtime = new StudioRuntimeClient(
    context,
    () => locator.runtimeExecutable(),
    externalTestApproval
      ? () => waitForExternalIntegrationApproval(externalTestApproval)
      : (pendingId) => reviewIntegrationEnrollment(client, pendingId),
    (confirmationId, sessionId) => confirmRuntimeForget(client, confirmationId, sessionId),
    (stage) => {
      initializationStage = `runtime:${stage}`;
    },
  );
  const state = new RuntimeState();
  const selection = new SelectionStore(context.globalStorageUri.fsPath);
  let lifecycle: Promise<void> = Promise.resolve();
  const afterReady = async <T>(action: () => Promise<T>): Promise<T> => {
    await lifecycle;
    return action();
  };
  let controller: Controller;
  const conversation = new ConversationView(
    context.extensionUri,
    (message) => {
      if (message.type === "prompt") {
        void run(() => afterReady(() => controller.prompt(message.text)));
      } else if (message.type === "answerApproval") {
        void run(() => afterReady(
          () => controller.answerApproval(message.approval, message.option, message.subjectDigest),
        ));
      } else if (message.type === "openWorkspace") {
        void run(() => afterReady(() => controller.openWorkspace()));
      } else if (message.type === "interrupt") {
        void run(() => afterReady(() => controller.interrupt()));
      } else {
        void run(() => afterReady(() => controller.close()));
      }
    },
    (session) => uniqueSessionTitle(session, state.sessions, state.providers),
    (visible) => controller.conversationVisibilityChanged(visible),
  );
  controller = new Controller(context, client, runtime, state, conversation, selection);
  const missionController = new MissionController(client, controller, state);
  const candidateController = new CandidateController(client);
  const missions = new MissionTree(missionController);
  const sessions = new SessionsTree(state);
  const providers = new ProvidersTree(state);

  context.subscriptions.push(
    state,
    controller,
    missionController,
    candidateController,
    conversation,
    missions,
    sessions,
    providers,
    vscode.window.registerTreeDataProvider("runtrol.sessions", sessions),
    vscode.window.registerTreeDataProvider("runtrol.missions", missions),
    vscode.window.registerTreeDataProvider("runtrol.providers", providers),
    vscode.workspace.registerTextDocumentContentProvider("runtrol-mission", missionController.documentProvider()),
    vscode.commands.registerCommand(
      "runtrol.refresh",
      () => run(() => afterReady(async () => Promise.all([controller.refresh(), missionController.refresh()]).then(() => undefined))),
    ),
    vscode.commands.registerCommand(
      "runtrol.validateMission",
      () => run(() => afterReady(() => missionController.validateMission())),
    ),
    vscode.commands.registerCommand(
      "runtrol.registerMissionGate",
      () => run(() => afterReady(() => missionController.registerGate())),
    ),
    vscode.commands.registerCommand(
      "runtrol.openMission",
      (item) => run(() => afterReady(() => missionController.openMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.startMission",
      (item) => run(() => afterReady(() => missionController.startMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.prepareMissionTask",
      (item) => run(() => afterReady(() => missionController.prepareTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.sendTaskInstruction",
      (item) => run(() => afterReady(() => missionController.sendTaskInstruction(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.verifyMissionTask",
      (item) => run(() => afterReady(() => missionController.verifyTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.retryMissionTask",
      (item) => run(() => afterReady(() => missionController.retryTask(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.pauseMission",
      (item) => run(() => afterReady(() => missionController.pauseMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.resumeMission",
      (item) => run(() => afterReady(() => missionController.resumeMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.cancelMission",
      (item) => run(() => afterReady(() => missionController.cancelMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.completeMissionIntegration",
      (item) => run(() => afterReady(() => missionController.completeIntegration(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.archiveMission",
      (item) => run(() => afterReady(() => missionController.archiveMission(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openTaskSession",
      (item) => run(() => afterReady(() => missionController.openTaskSession(item))),
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
      "runtrol.switchSession",
      () => run(() => afterReady(() => controller.switchSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.startSession",
      () => run(() => afterReady(() => controller.startSession())),
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

  lifecycle = runtime.initialize()
    .then(() => {
      initializationStage = "controller";
      return controller.initialize();
    })
    .then(() => {
      initializationStage = "mission";
      return missionController.initialize();
    })
    .then(() => {
      initializationStage = "ready";
    });
  void run(() => lifecycle);
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
        await controller.select(cold.sessionId, false);
        await Promise.all([
          controller.selectedWatchReady(),
          conversation.waitForCurrentRender(),
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
        for (let round = 0; round < 2; round += 1) {
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

async function waitForExternalIntegrationApproval(marker: string): Promise<boolean> {
  if (!path.isAbsolute(marker)) {
    throw new Error("the external integration approval marker must be absolute");
  }
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const integrationId = (await readFile(marker, "utf8")).trim();
      if (!/^int_[0-9a-f]{32}$/u.test(integrationId)) {
        throw new Error("the external integration approval marker is malformed");
      }
      return true;
    } catch (error) {
      if (!isMissingFile(error)) throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("the external integration approval did not complete in time");
}

function isMissingFile(error: unknown): boolean {
  return Boolean(error && typeof error === "object" && "code" in error
    && (error as NodeJS.ErrnoException).code === "ENOENT");
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
    await vscode.window.showErrorMessage(`Runtrol: ${error instanceof Error ? error.message : String(error)}`);
  }
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
