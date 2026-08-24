import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

/// The focused conversation-management eye pass: the whole point of the sidebar in five pictures. A coding
/// service that ships no delete or rename command of its own (Claude) still shows its stored conversations by
/// name, and one conversation is managed in place from its row: renamed to a name the operator chose, pinned to
/// the top, and deleted.
///
/// It proves this on a throwaway conversation it starts and one plain reply fills, so it renames, pins and
/// deletes only what it made and never touches a conversation the operator would miss. The rename and the pin are
/// Runtrol's own local choices (instant, no conversation reopened); the delete goes through the service's store.
type ConversationJourney = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  start(provider: string, workspace: string): Promise<string>;
  verifySelected(session: string): Promise<void>;
  prompt(text: string): Promise<void>;
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
  close(session: string, now?: boolean): Promise<void>;
  refreshChats(): Promise<void>;
  nativeChatCount(): number;
  conversationTitles(): readonly string[];
  nativeChatListed(providerId: string, nativeSessionId: string): boolean;
  canDeleteNative(providerId: string): boolean;
  revealConversation(providerId: string, nativeSessionId: string): Promise<void>;
  nameListed(providerId: string, nativeSessionId: string, label: string): Promise<void>;
  pinListed(providerId: string, nativeSessionId: string): Promise<void>;
  deleteNativeListed(providerId: string, nativeSessionId: string): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: ConversationJourney;
};

const RENAMED = "Renamed from the sidebar";

let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await eyePass(resultPath);
  } catch (error) {
    await writeResult(resultPath, {
      stage: currentStage,
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    });
    throw error;
  }
}

async function eyePass(resultPath: string): Promise<void> {
  const providerId = requiredEnvironment("RUNTROL_EYE_PROVIDER");

  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await within(vscode.commands.executeCommand("workbench.view.extension.runtrol"), 30_000, "opening Runtrol");
  while (!extension.isActive) await delay(25);
  const api = extension.exports;
  await within(api.ready, 60_000, "extension initialization");
  const journey = api.journey;
  if (!journey) throw new Error("the journey API is unavailable");

  currentStage = "waiting-for-service";
  await waitFor(
    () => journey.providers().some((provider) => (
      provider.providerId === providerId && provider.installation.state === "usable"
    )),
    120_000,
    `the installed service ${providerId}`,
  ).catch((error: unknown) => {
    const seen = journey.providers().map((provider) => (
      `${provider.providerId}:${JSON.stringify(provider.installation)}`
    )).join(", ");
    throw new Error(`${error instanceof Error ? error.message : String(error)}; services seen: ${seen}`);
  });

  // A throwaway conversation of the service, filled by one plain reply that writes nothing, so the service
  // stores it (a conversation with no turn has no stored rollout to name or delete). Everything after happens to
  // this one row.
  currentStage = "starting-throwaway";
  const scratch = await mkdtemp(path.join(os.tmpdir(), "runtrol-conv-eye-"));
  try {
    const session = await within(journey.start(providerId, scratch), 60_000, "starting the throwaway conversation");
    await journey.verifySelected(session);
    await journey.prompt("Reply with exactly: ok");
    await journey.waitForLifecycle(session, "hotRunning", 30_000).catch(() => undefined);
    await within(journey.waitForLifecycle(session, "hotIdle", 240_000), 240_000, "the reply to finish");
    const native = journey.sessions().find((line) => line.sessionId === session)?.nativeSessionId ?? null;
    if (!native) throw new Error("the throwaway conversation announced no native identity to manage");

    currentStage = "listing-conversations";
    await within(journey.refreshChats(), 90_000, "refreshing stored conversations");
    await waitFor(() => journey.nativeChatListed(providerId, native), 60_000, "the throwaway conversation in the list");
    await waitFor(() => journey.canDeleteNative(providerId), 30_000, `the ${providerId} deletion surface`);

    // Pose 1: names. The service's stored conversations show by the names it kept, not internal identifiers. A
    // service with no rename command of its own still names its history.
    const named = journey.conversationTitles().filter((title) => title && title !== "Unnamed conversation");
    if (named.length === 0) {
      throw new Error(`no conversation in the sidebar shows a name; titles: ${journey.conversationTitles().join(" | ")}`);
    }
    await settle();
    await capture(resultPath, "names", { conversations: journey.nativeChatCount(), named: named.length });

    // Pose 2: the row's own management. The throwaway's row is brought forward so its inline actions show: the
    // pencil that renames, the pin that keeps it up top, and the cross that deletes. All three on a row of a
    // service that ships none of these commands itself.
    await journey.revealConversation(providerId, native);
    await settle();
    await capture(resultPath, "rowActions", { managed: native });

    // Pose 3: rename. The row takes the operator's name at once, without reopening the conversation or asking the
    // service; the row now reads the chosen name.
    currentStage = "renaming";
    await within(journey.nameListed(providerId, native, RENAMED), 30_000, "renaming from the sidebar");
    await within(journey.refreshChats(), 90_000, "listing after the rename");
    if (!journey.conversationTitles().includes(RENAMED)) {
      throw new Error(`the renamed row does not read "${RENAMED}"; titles: ${journey.conversationTitles().join(" | ")}`);
    }
    await journey.revealConversation(providerId, native);
    await settle();
    await capture(resultPath, "renamed", { now: RENAMED });

    // Pose 4: pin. The pinned conversation leads the whole list, ahead of every more recently touched one,
    // because pinning is a placement the operator chose.
    currentStage = "pinning";
    await within(journey.pinListed(providerId, native), 30_000, "pinning from the sidebar");
    await within(journey.refreshChats(), 90_000, "listing after the pin");
    const order = journey.conversationTitles();
    if (order[0] !== RENAMED) {
      throw new Error(`the pinned conversation is not first; order: ${order.slice(0, 6).join(" | ")}`);
    }
    await journey.revealConversation(providerId, native);
    await settle();
    await capture(resultPath, "pinned", { pinned: RENAMED });

    // Pose 5: delete. The conversation is handed back to the service (closed and forgotten here), then deleted
    // through the service's own store from its row, and the list no longer names it (a fact the store answers).
    currentStage = "deleting";
    await within(journey.close(session, true), 60_000, "forgetting the throwaway conversation");
    await within(journey.refreshChats(), 90_000, "listing before the delete");
    if (!journey.nativeChatListed(providerId, native)) {
      throw new Error("the throwaway conversation left the list before it was deleted");
    }
    await within(journey.deleteNativeListed(providerId, native), 60_000, "deleting from the sidebar");
    await within(journey.refreshChats(), 90_000, "listing after the delete");
    if (journey.nativeChatListed(providerId, native)) {
      throw new Error("the deleted conversation is still listed");
    }
    await settle();
    await capture(resultPath, "deleted", { deleted: native, remaining: journey.nativeChatCount() });

    await writeResult(resultPath, { stage: "complete", vscode: vscode.version, renamed: RENAMED });
  } finally {
    await rm(scratch, { recursive: true, force: true }).catch(() => undefined);
  }
}

/// A breath for the tree to repaint and any toast to clear, then a still window to photograph.
async function settle(): Promise<void> {
  await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar").then(undefined, () => undefined);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  await delay(1_500);
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeResult(resultPath, { stage: `capture:${pose}`, ...facts });
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await vscode.workspace.fs.stat(vscode.Uri.file(confirmation));
      return;
    } catch {
      if (Date.now() >= deadline) throw new Error(`the photographer did not capture ${pose}`);
      await delay(100);
    }
  }
}

async function writeResult(resultPath: string, value: Record<string, unknown>): Promise<void> {
  await vscode.workspace.fs.writeFile(vscode.Uri.file(resultPath), Buffer.from(JSON.stringify(value), "utf8"));
}

async function waitFor(predicate: () => boolean, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await delay(100);
  }
}

function within<T>(promise: Thenable<T> | Promise<T>, deadlineMs: number, label: string): Promise<T> {
  return Promise.race([
    Promise.resolve(promise),
    new Promise<T>((_resolve, reject) => {
      setTimeout(() => reject(new Error(`${label} exceeded ${deadlineMs} ms`)), deadlineMs);
    }),
  ]);
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
