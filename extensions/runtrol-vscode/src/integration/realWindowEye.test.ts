import { readFile, writeFile } from "node:fs/promises";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

/// The real-window eye pass.
///
/// Everything in the picture is real: the installed CLIs on this machine, the folders they ran in, the
/// conversations they stored, a conversation started and answered by the installed provider in this very
/// repository. Only the window is isolated (its own user data and its own Runtime home), so the operator's
/// own window and daemon are never touched. The harness outside photographs each pose when this entry says
/// it is standing still; the pictures are the judgement, not this file.
type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  start(provider: string, workspace: string): Promise<string>;
  prompt(text: string): Promise<void>;
  close(session: string, now?: boolean): Promise<void>;
  verifySelected(session: string): Promise<void>;
  openDraft(workspace: string | null, providerId?: string): Promise<void>;
  sendFocusedDraft(text: string): Promise<string>;
  openListed(limit: number): Promise<{ opened: number; refused: string[] }>;
  nativeChatCount(): number;
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: JourneyApi;
};

let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await eyePass(resultPath);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        stage: currentStage,
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
      }),
      "utf8",
    );
    throw error;
  }
}

async function eyePass(resultPath: string): Promise<void> {
  const folder = requiredEnvironment("RUNTROL_EYE_FOLDER");
  const providerId = requiredEnvironment("RUNTROL_EYE_PROVIDER");
  const promptText = process.env.RUNTROL_EYE_PROMPT
    || "Reply in one short line: name this folder and count the entries at its root. Use your tools to look.";
  const tabsWanted = Number(process.env.RUNTROL_EYE_TABS || "10");

  currentStage = "activating";
  const extension = extensionUnderTest<ExtensionApi>();
  await within(vscode.commands.executeCommand("workbench.view.extension.runtrol"), 30_000, "opening the Runtrol view");
  while (!extension.isActive) await delay(25);
  const api = extension.exports;
  await within(api.ready, 60_000, "extension initialization");
  const journey = api.journey;
  if (!journey) throw new Error("the journey API is unavailable");

  currentStage = "waiting-for-providers";
  await waitFor(
    () => journey.providers().some((provider) => provider.providerId === providerId && provider.installation.state === "usable"),
    90_000,
    `the installed provider ${providerId} to verify`,
  );
  // The machine's stored conversations arrive one service at a time. A bounded wait for the first of them,
  // then a breath for the rest; a machine with none would still photograph honestly (empty headings are
  // never invented, so the panel would simply be short).
  currentStage = "waiting-for-catalogues";
  await waitFor(() => journey.nativeChatCount() > 0, 90_000, "the first stored conversations").catch(() => undefined);
  await delay(6_000);

  // The editor's own furniture out of the picture: the built-in chat sidebar the isolated profile opens, and
  // the "extensions are disabled" toast the development host shows. Neither is this product.
  await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar").then(undefined, () => undefined);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);

  // Pose 1: a draft on this folder, the panel beside it.
  currentStage = "draft";
  await journey.openDraft(folder, providerId);
  await delay(1_500);
  await capture(resultPath, "draft", { nativeChats: journey.nativeChatCount() });

  // Pose 2: the draft's first message starts a real conversation in this repository, in the same tab,
  // answered by the installed provider, tools and all.
  currentStage = "sending-draft";
  const session = await within(journey.sendFocusedDraft(promptText), 90_000, "sending the draft's first message");
  await journey.verifySelected(session);
  currentStage = "answering";
  await journey.waitForLifecycle(session, "hotRunning", 30_000).catch(() => undefined);
  await journey.waitForLifecycle(session, "hotIdle", 240_000);
  await delay(1_200);
  await capture(resultPath, "conversation", { session });

  // Pose 3: many conversation tabs, the editor's own groups doing the arranging.
  currentStage = "opening-tabs";
  const listed = await journey.openListed(Math.max(0, tabsWanted - 1));
  await delay(1_000);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  // More groups, so the picture shows the editor splitting and sizing them itself: only when there are
  // tabs to move, and always with a conversation tab focused so an empty group is never created.
  const conversationTabs = (): vscode.Tab[] => vscode.window.tabGroups.all
    .flatMap((group) => group.tabs)
    .filter((tab) => tab.input instanceof vscode.TabInputWebview);
  if (conversationTabs().length >= 3) {
    await vscode.commands.executeCommand("workbench.action.focusFirstEditorGroup");
    await vscode.commands.executeCommand("workbench.action.moveEditorToNextGroup");
    await vscode.commands.executeCommand("workbench.action.focusFirstEditorGroup");
    await vscode.commands.executeCommand("workbench.action.moveEditorToNextGroup");
    if (conversationTabs().length >= 5) {
      await vscode.commands.executeCommand("workbench.action.focusSecondEditorGroup");
      await vscode.commands.executeCommand("workbench.action.moveEditorToNextGroup");
    }
    await vscode.commands.executeCommand("workbench.action.evenEditorWidths");
  }
  await delay(2_000);
  const tabs = conversationTabs().length;
  await capture(resultPath, "tabs", {
    opened: listed.opened + 1,
    refused: listed.refused,
    tabs,
    groups: vscode.window.tabGroups.all.length,
  });

  // Leave nothing running: every session this window opened closes, in the isolated Runtime.
  currentStage = "closing";
  for (const line of journey.sessions()) {
    await journey.close(line.sessionId, true).catch(() => undefined);
  }
  await writeFile(
    resultPath,
    JSON.stringify({ stage: "complete", vscode: vscode.version, tabs, opened: listed.opened, refused: listed.refused }),
    "utf8",
  );
}

/// Hold still and let the harness photograph; continue when it says it has.
async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}`, ...facts }), "utf8");
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await readFile(confirmation, "utf8");
      return;
    } catch {
      // Not photographed yet.
    }
    if (Date.now() > deadline) throw new Error(`the harness never confirmed the ${pose} capture`);
    await delay(250);
  }
}

async function waitFor(condition: () => boolean, deadlineMs: number, label: string): Promise<void> {
  const deadline = Date.now() + deadlineMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await delay(100);
  }
}

async function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      Promise.resolve(work),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
