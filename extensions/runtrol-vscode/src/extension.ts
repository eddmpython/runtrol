import * as vscode from "vscode";

import { ConversationView } from "./conversationView";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import { RuntimeState } from "./state";
import { ProvidersTree, SessionsTree } from "./trees";

export function activate(context: vscode.ExtensionContext): void {
  const locator = new CoreLocator(context);
  const client = new CoreClient(locator);
  const state = new RuntimeState();
  let controller: Controller;
  const conversation = new ConversationView(context.extensionUri, (message) => {
    if (message.type === "prompt") {
      void run(() => controller.prompt(message.text));
    } else if (message.type === "answerApproval") {
      void run(() => controller.answerApproval(message.approval, message.option, message.subjectDigest));
    } else if (message.type === "openWorkspace") {
      void run(() => controller.openWorkspace());
    } else if (message.type === "interrupt") {
      void run(() => controller.interrupt());
    } else {
      void run(() => controller.close());
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
    vscode.commands.registerCommand("runtrol.refresh", () => run(() => controller.refresh())),
    vscode.commands.registerCommand("runtrol.startSession", () => run(() => controller.startSession())),
    vscode.commands.registerCommand("runtrol.selectSession", (item) => run(() => controller.select(item))),
    vscode.commands.registerCommand("runtrol.openWorkspace", (item) => run(() => controller.openWorkspace(item))),
    vscode.commands.registerCommand("runtrol.interrupt", () => run(() => controller.interrupt())),
    vscode.commands.registerCommand("runtrol.closeSession", (item) => run(() => controller.close(item))),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("runtrol.corePath")) {
        locator.invalidate();
        void run(() => controller.refresh());
      }
    }),
  );

  void run(() => controller.initialize());
}

export function deactivate(): void {}

async function run(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    await vscode.window.showErrorMessage(`Runtrol: ${error instanceof Error ? error.message : String(error)}`);
  }
}
