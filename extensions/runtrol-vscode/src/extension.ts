import * as vscode from "vscode";

import { ConversationView, type WebviewPerformance } from "./conversationView";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import { RuntimeState } from "./state";
import { ProvidersTree, SessionsTree } from "./trees";

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<WebviewPerformance>;
};

export function activate(context: vscode.ExtensionContext): RuntrolExtensionApi {
  const locator = new CoreLocator(context);
  const client = new CoreClient(locator);
  const state = new RuntimeState();
  let lifecycle: Promise<void> = Promise.resolve();
  const afterReady = async <T>(action: () => Promise<T>): Promise<T> => {
    await lifecycle;
    return action();
  };
  let controller: Controller;
  const conversation = new ConversationView(context.extensionUri, (message) => {
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
  });
  controller = new Controller(context, client, state, conversation);
  const sessions = new SessionsTree(state);
  const providers = new ProvidersTree(state);

  context.subscriptions.push(
    state,
    controller,
    sessions,
    providers,
    vscode.window.registerTreeDataProvider("runtrol.sessions", sessions),
    vscode.window.registerTreeDataProvider("runtrol.providers", providers),
    vscode.window.registerWebviewViewProvider(ConversationView.viewType, conversation, {
      webviewOptions: { retainContextWhenHidden: false },
    }),
    vscode.commands.registerCommand("runtrol.refresh", () => run(() => afterReady(() => controller.refresh()))),
    vscode.commands.registerCommand(
      "runtrol.startSession",
      () => run(() => afterReady(() => controller.startSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.selectSession",
      (item) => run(() => afterReady(() => controller.select(item))),
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

  lifecycle = controller.initialize();
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
