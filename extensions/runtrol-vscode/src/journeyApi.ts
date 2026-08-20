import * as vscode from "vscode";

import type { ConversationPanels } from "./conversationPanels";
import { Controller } from "./controller";
import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { RuntimeState } from "./state";

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
};

export function journeyApi(
  controller: Controller,
  state: RuntimeState,
  conversation: ConversationPanels,
  afterReady: <T>(action: () => Promise<T>) => Promise<T>,
  extensionMode: vscode.ExtensionMode,
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
    reconnect: () => afterReady(async () => {
      await controller.reconnect();
      await controller.selectedWatchReady();
    }),
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
  };
}
