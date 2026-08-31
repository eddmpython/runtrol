import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import { Watcher } from "./realProviderWatch.test";
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
  select(session: string): Promise<void>;
  answerApproval(approval: string, option: number, subjectDigest: number[]): Promise<void>;
  verifySelected(session: string): Promise<void>;
  openListed(limit: number): Promise<{ opened: number; refused: string[] }>;
  openStoredWithTitle(providerId: string): Promise<string | null>;
  rowTool(session: string): string | null;
  answerFromRow(session: string, how: "allow" | "decline"): Promise<void>;
  revealRow(session: string): Promise<void>;
  nativeChatCount(): number;
  canDeleteNative(providerId: string): boolean;
  nativeChatListed(providerId: string, nativeSessionId: string): boolean;
  deleteNativeListed(providerId: string, nativeSessionId: string): Promise<void>;
  refreshChats(): Promise<void>;
  waitForLifecycle(session: string, lifecycle: SessionLine["lifecycle"], deadlineMs: number): Promise<void>;
  terminalStart(providerId: string, workspace: string, deadlineMs: number): Promise<{
    runtimeGeneration: string;
    terminalId: string;
  }>;
  terminalStop(runtimeGeneration: string, terminalId: string, deadlineMs: number): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: JourneyApi;
  /// Opens the first stored conversation the tree shows, through the same path a click takes.
  openFirstConversation?(): Promise<void>;
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
  const createdSessions = new Set<string>();

  // The editor's own furniture out of the picture: the built-in chat sidebar the isolated profile opens, and
  // the "extensions are disabled" toast the development host shows. Neither is this product.
  await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar").then(undefined, () => undefined);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);

  // The shortest truthful visual pass: open a fresh provider TUI for this folder, send no model turn, capture
  // the real Studio header and terminal, then stop only that exact throwaway process. No provider transcript is
  // created because no prompt is sent.
  if (process.env.RUNTROL_EYE_DRAFT_ONLY === "1") {
    currentStage = "draft";
    const terminal = await within(
      journey.terminalStart(providerId, folder, 60_000),
      70_000,
      "opening the current-folder provider terminal",
    );
    await delay(4_000);
    await capture(resultPath, "draft", { ...terminal, providerId });
    await within(
      journey.terminalStop(terminal.runtimeGeneration, terminal.terminalId, 60_000),
      70_000,
      "closing the current-folder provider terminal",
    );
    await writeFile(
      resultPath,
      JSON.stringify({ stage: "complete", focused: "draft", providerId, vscode: vscode.version }),
      "utf8",
    );
    return;
  }

  await within(journey.refreshChats(), 90_000, `refreshing stored conversations for ${providerId}`);
  // The machine's stored conversations arrive one service at a time. A bounded wait for the first of them,
  // then a breath for the rest; a machine with none would still photograph honestly (empty headings are
  // never invented, so the panel would simply be short).
  currentStage = "waiting-for-catalogues";
  await waitFor(() => journey.nativeChatCount() > 0, 90_000, "the first stored conversations").catch(() => undefined);
  await delay(6_000);
  await waitFor(
    () => journey.canDeleteNative(providerId),
    60_000,
    `a provider-owned cleanup surface for ${providerId}`,
  );

  // Pose 1: the conversation surface itself. The first stored conversation opens as the service's own
  // terminal interface in an editor tab (the Core hosts the CLI on a pseudo terminal); the photograph is
  // taken once the CLI has had time to draw its screen.
  currentStage = "terminal";
  if (!api.openFirstConversation) throw new Error("the conversation opener is unavailable");
  await within(api.openFirstConversation(), 60_000, "opening the first stored conversation");
  await delay(Number(process.env.RUNTROL_EYE_TERMINAL_SETTLE_MS || "10000"));
  await capture(resultPath, "terminal", { nativeChats: journey.nativeChatCount() });
  if (process.env.RUNTROL_EYE_TERMINAL_ONLY === "1") {
    await writeFile(
      resultPath,
      JSON.stringify({ stage: "complete", focused: "terminal", vscode: vscode.version }),
      "utf8",
    );
    return;
  }

  // Pose 2: many conversation tabs, each a service's own screen, spread over the editor's groups by the one
  // grid command. The editor does the splitting and sizing; the pass only asks.
  currentStage = "opening-tabs";
  const listed = await journey.openListed(Math.max(0, tabsWanted - 1));
  await delay(4_000);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  const conversationTabs = (): vscode.Tab[] => vscode.window.tabGroups.all
    .flatMap((group) => group.tabs)
    .filter((tab) => tab.input instanceof vscode.TabInputTerminal);
  await vscode.commands.executeCommand("runtrol.arrangeConversationGrid").then(undefined, () => undefined);
  await delay(3_000);
  const tabs = conversationTabs().length;
  await capture(resultPath, "tabs", {
    opened: listed.opened + 1,
    refused: listed.refused,
    tabs,
    groups: vscode.window.tabGroups.all.length,
  });

  // Pose 3: a stored conversation of the provider, reopened from the sidebar: its history shows the way the
  // service's own resume draws it, in its own terminal.
  currentStage = "reopening";
  await vscode.commands.executeCommand("workbench.action.editorLayoutSingle").then(undefined, () => undefined);
  const reopened = await within(journey.openStoredWithTitle(providerId), 60_000, "reopening a stored conversation");
  await delay(6_000);
  await capture(resultPath, "reopened", { reopened });

  // Pose 7: a throwaway conversation of a provider that can delete is started in a scratch folder and asked
  // to write a file, so the service declares a change: the tab names it with an "Open diff" button, and the
  // change opens in VS Code's own diff editor (two pictures). Then the conversation is handed back to the
  // provider (closed and forgotten here), deleted through the provider's own surface from the sidebar's row,
  // and the provider's list no longer names it (a fact). Real CLI, real store, a conversation nobody will
  // miss. Skipped when no installed provider can delete.
  // scratch folder, handed back to the provider (closed and forgotten here), then deleted through the
  // provider's own surface from the sidebar's row, and the provider's list no longer names it. Real CLI,
  // real store, a conversation nobody will miss. Skipped when no installed provider can delete.
  currentStage = "deleting";
  const deletion = await deletionProof(
    journey,
    providerIdForDeletion(journey, journey.providers()),
    resultPath,
  );

  // Leave nothing running or stored: every new provider conversation is captured before its Runtime pointer
  // closes, then removed through the provider's own published deletion surface.
  currentStage = "closing";
  const createdNative = journey.sessions()
    .filter((line) => createdSessions.has(line.sessionId) && line.nativeSessionId)
    .map((line) => ({ providerId: line.providerId, nativeSessionId: line.nativeSessionId as string }));
  for (const line of journey.sessions()) {
    await journey.close(line.sessionId, true).catch(() => undefined);
  }
  await within(journey.refreshChats(), 90_000, "listing conversations before eye cleanup");
  for (const created of createdNative) {
    if (!journey.nativeChatListed(created.providerId, created.nativeSessionId)) continue;
    await within(
      journey.deleteNativeListed(created.providerId, created.nativeSessionId),
      90_000,
      `deleting eye conversation ${created.nativeSessionId}`,
    );
    if (journey.nativeChatListed(created.providerId, created.nativeSessionId)) {
      throw new Error(`eye conversation ${created.nativeSessionId} remained in provider history`);
    }
  }
  await writeFile(
    resultPath,
    JSON.stringify({
      stage: "complete",
      vscode: vscode.version,
      tabs,
      opened: listed.opened,
      refused: listed.refused,
      reopened,
      deletion,
      cleanedProviderConversations: createdNative.length,
    }),
    "utf8",
  );
}

/// The first usable provider whose current Runtime capability report exposes native deletion.
function providerIdForDeletion(
  journey: JourneyApi,
  providers: readonly ProviderLine[],
): string | null {
  return providers.find((provider) => (
    provider.installation.state === "usable" && journey.canDeleteNative(provider.providerId)
  ))?.providerId ?? null;
}

async function deletionProof(
  journey: JourneyApi,
  providerId: string | null,
  resultPath: string,
): Promise<Record<string, unknown>> {
  if (!providerId) return { skipped: "no installed provider publishes a deletion surface" };
  const scratch = await mkdtemp(path.join(os.tmpdir(), "runtrol-eye-delete-"));
  try {
    const session = await within(journey.start(providerId, scratch), 60_000, "starting the throwaway conversation");
    await waitFor(
      () => Boolean(journey.sessions().find((line) => line.sessionId === session)?.nativeSessionId),
      60_000,
      "the throwaway conversation's provider identity",
    );
    const native = journey.sessions().find((line) => line.sessionId === session)?.nativeSessionId;
    if (!native) throw new Error("the throwaway conversation announced no native identity after the wait");
    // One real turn, because the provider stores a thread only once something was said (measured on
    // Codex 0.148: a fresh thread with no turn has no rollout to list or delete). The turn writes a file,
    // so the service declares a change and the diff poses have something real to show.
    await journey.verifySelected(session);
    // The service asks before it writes (default-deny); the pass answers each question with the service's own
    // first allow option, exactly as a person clicking the card would, through the same approval path.
    const watch = new Watcher(requiredEnvironment("RUNTROL_TEST_CORE"), session);
    await watch.ready;
    let approvalsAnswered = 0;
    let approvalsUnanswerable = 0;
    let answeredFromRow = false;
    const answering = (async () => {
      for (;;) {
        const fact = await watch.next("approval", 300_000).catch(() => null);
        if (!fact) return;
        if (fact.allow === null) {
          approvalsUnanswerable += 1;
          continue;
        }
        if (!answeredFromRow) {
          // The first question is answered from the sidebar row, as a person would without opening the tab:
          // the row says "Needs you", its inline allow is pressed, and the row changes. Photographed first.
          answeredFromRow = true;
          await delay(1_200);
          await journey.revealRow(session);
          await capture(resultPath, "rowApproval", { session });
          await journey.answerFromRow(session, "allow");
          approvalsAnswered += 1;
          continue;
        }
        await journey.answerApproval(fact.approval, fact.allow, fact.subjectDigest);
        approvalsAnswered += 1;
      }
    })();
    await journey.prompt("Create a file named hello.txt in this folder containing exactly the word hi, then reply: done");
    await journey.waitForLifecycle(session, "hotRunning", 30_000).catch(() => undefined);
    await journey.waitForLifecycle(session, "hotIdle", 240_000);
    await watch.stop().catch(() => undefined);
    await answering.catch(() => undefined);
    await vscode.commands.executeCommand("workbench.action.editorLayoutSingle").then(undefined, () => undefined);
    await journey.select(session);
    await delay(1_500);
    await capture(resultPath, "answered", { session, approvalsAnswered, approvalsUnanswerable });
    // Handed back to the provider: closed and forgotten here, so the row is the provider's alone.
    await within(journey.close(session, true), 60_000, "forgetting the throwaway conversation");
    await within(journey.refreshChats(), 90_000, "listing after the close");
    const listedBefore = journey.nativeChatListed(providerId, native);
    await within(journey.deleteNativeListed(providerId, native), 90_000, "deleting through the provider");
    const listedAfter = journey.nativeChatListed(providerId, native);
    return {
      providerId,
      native,
      listedBefore,
      listedAfter,
      deleted: listedBefore && !listedAfter,
      approvalsAnswered,
      approvalsUnanswerable,
    };
  } finally {
    await rm(scratch, { recursive: true, force: true }).catch(() => undefined);
  }
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
