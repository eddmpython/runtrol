import * as vscode from "vscode";

import { ConversationView, type WebviewPerformance } from "./conversationView";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import {
  manageIntegrations,
  reviewIntegrationEnrollment,
  reviewIntegrationEnrollments,
  reviewRuntimeRequests,
} from "./integrationAdministration";
import { journeyApi, type JourneyApi } from "./journeyApi";
import { SelectionStore } from "./selectionStore";
import { uniqueSessionTitle } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { ProvidersTree, SessionsTree } from "./trees";

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
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
  const runtime = new StudioRuntimeClient(
    context,
    () => client.ensureRuntime(),
    (pendingId) => reviewIntegrationEnrollment(client, pendingId),
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
  const sessions = new SessionsTree(state);
  const providers = new ProvidersTree(state);

  context.subscriptions.push(
    state,
    controller,
    conversation,
    sessions,
    providers,
    vscode.window.registerTreeDataProvider("runtrol.sessions", sessions),
    vscode.window.registerTreeDataProvider("runtrol.providers", providers),
    vscode.commands.registerCommand("runtrol.refresh", () => run(() => afterReady(() => controller.refresh()))),
    vscode.commands.registerCommand(
      "runtrol.checkProviderUpdates",
      () => run(() => afterReady(() => controller.checkProviderUpdates())),
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
      () => run(() => afterReady(async () => controller.openConversation())),
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
        void run(() => lifecycle);
      }
    }),
  );

  lifecycle = runtime.initialize().then(() => controller.initialize());
  void run(() => lifecycle);
  return {
    get ready() {
      return lifecycle;
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
        const managed = state.sessions.filter((session) => expected.has(session.session));
        const initialHot = managed.filter((session) => session.hot);
        const cold = managed.find((session) => !session.hot);
        if (expected.size !== 30 || managed.length !== expected.size || initialHot.length !== 8 || !cold) {
          throw new Error(
            `expected 30 named sessions with eight hot and a cold choice, found ${managed.length} and ${initialHot.length}`,
          );
        }

        const resumeStarted = performance.now();
        await controller.select(cold.session, false);
        await Promise.all([
          controller.selectedWatchReady(),
          conversation.waitForCurrentRender(),
        ]);
        const coldResumeMs = performance.now() - resumeStarted;
        const resumed = state.selected;
        if (
          !resumed
          || !resumed.hot
          || resumed.session === cold.session
          || resumed.provider !== cold.provider
          || resumed.native !== cold.native
          || resumed.workspace !== cold.workspace
          || state.sessions.some((session) => session.session === cold.session)
        ) {
          throw new Error("selecting a cold row did not replace it with the same provider-owned hot session");
        }
        const current = state.sessions.filter(
          (session) => expected.has(session.session) || session.session === resumed.session,
        );
        const hot = current.filter((session) => session.hot);
        if (current.length !== 30 || hot.length !== 8) {
          throw new Error(`cold resume changed the 30-session and eight-hot bounds to ${current.length} and ${hot.length}`);
        }
        const samples: number[] = [];
        for (let round = 0; round < 2; round += 1) {
          for (const session of hot) {
            const started = performance.now();
            await controller.select(session.session, false);
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
          resumedFrom: cold.session,
          resumedTo: resumed.session,
          restoreSession: state.selected?.session ?? "",
          restoreWorkspace: state.selected?.workspace ?? "",
        };
      })
      : undefined,
    verifyRestoredSession: process.env.RUNTROL_VSCODE_PERFORMANCE === "1"
      ? (sessionId) => afterReady(async () => {
        if (state.selected?.session !== sessionId) {
          throw new Error(`restored ${state.selected?.session ?? "no session"}, expected ${sessionId}`);
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

export function deactivate(): void {}

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
