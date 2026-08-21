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
  openDraft(workspace: string | null, providerId?: string): Promise<void>;
  sendFocusedDraft(text: string): Promise<string>;
  openListed(limit: number): Promise<{ opened: number; refused: string[] }>;
  placeConversation(session: string, place: "tab" | "panel" | "sideBar"): Promise<void>;
  arrangeGrid(): Promise<{ arranged: number; leftInPlace: number }>;
  openStoredWithTitle(providerId: string): Promise<string | null>;
  openLatestDiff(): Promise<void>;
  clickChip(anchor: "project" | "service" | "model" | "effort" | "mode"): Promise<void>;
  alsoAsk(providerId: string): Promise<void>;
  rowTool(session: string): string | null;
  answerFromRow(session: string, how: "allow" | "decline"): Promise<void>;
  revealRow(session: string): Promise<void>;
  nativeChatCount(): number;
  nativeChatListed(providerId: string, nativeSessionId: string): boolean;
  deleteNativeListed(providerId: string, nativeSessionId: string): Promise<void>;
  refreshChats(): Promise<void>;
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

  // Pose 1b: the model chip's choices, open in the composer where the chip is (not a palette at the top of
  // the window). The service chip's choices the same way afterwards, then both closed.
  currentStage = "menu";
  await journey.clickChip("model");
  await delay(1_500);
  await capture(resultPath, "menu", { anchor: "model" });
  await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup").then(undefined, () => undefined);
  await journey.clickChip("service");
  await delay(1_200);
  await capture(resultPath, "menuService", { anchor: "service" });
  await journey.clickChip("service");
  await delay(600);

  // Pose 2: the draft's first message starts a real conversation in this repository, in the same tab,
  // answered by the installed provider, tools and all.
  currentStage = "sending-draft";
  const session = await within(journey.sendFocusedDraft(promptText), 90_000, "sending the draft's first message");
  await journey.verifySelected(session);
  currentStage = "answering";
  await journey.waitForLifecycle(session, "hotRunning", 30_000).catch(() => undefined);
  // Pose 2a: the row names the tool the service says is running, while it runs, in the service's own word.
  // Bounded wait: a fast answer may finish before any tool is named, and that is recorded, not forced.
  const toolDeadline = Date.now() + 40_000;
  let rowTool: string | null = null;
  while (Date.now() < toolDeadline) {
    rowTool = journey.rowTool(session);
    if (rowTool) break;
    await delay(200);
  }
  if (rowTool) {
    await journey.revealRow(session);
    await capture(resultPath, "activity", { session, rowTool });
  }
  await journey.waitForLifecycle(session, "hotIdle", 240_000);
  await delay(1_200);
  await capture(resultPath, "conversation", { session });

  // Pose 2b: one prompt to two services. A draft on the same folder with a second service added ("also ask"),
  // sent once; two tabs start, the grid lines them up. Skipped when only one service is usable.
  currentStage = "fan-out";
  const second = journey.providers().find(
    (provider) => provider.providerId !== providerId && provider.installation.state === "usable",
  );
  let fanOut: Record<string, unknown> = { skipped: "one usable service" };
  if (second) {
    await journey.openDraft(folder, providerId);
    await delay(800);
    await journey.alsoAsk(second.providerId);
    await delay(600);
    await capture(resultPath, "fanOutDraft", { also: second.providerId });
    let first: string | null = null;
    let failure: string | null = null;
    try {
      first = await within(journey.sendFocusedDraft("Reply with exactly: ok"), 180_000, "sending the fan-out draft");
    } catch (error) {
      failure = error instanceof Error ? error.message : String(error);
    }
    await delay(4_000);
    const started = journey.sessions().filter((line) => line.hot).length;
    // Named sendFailure, not failure: the photographer reads a top-level "failure" as the pass having failed.
    await capture(resultPath, "fanOut", { first, also: second.providerId, hot: started, sendFailure: failure });
    fanOut = { first, also: second.providerId, hot: started, sendFailure: failure };
    for (const line of journey.sessions()) {
      if (line.sessionId !== session) {
        await journey.waitForLifecycle(line.sessionId, "hotIdle", 120_000).catch(() => undefined);
      }
    }
  }

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

  // Pose 4: a stored conversation of the provider, reopened from the sidebar: its history shows, the way the
  // provider's own resume draws it (Codex from its resume answer, Claude Code from its own store).
  currentStage = "reopening";
  await vscode.commands.executeCommand("workbench.action.editorLayoutSingle").then(undefined, () => undefined);
  const reopened = await within(journey.openStoredWithTitle(providerId), 60_000, "reopening a stored conversation");
  await delay(2_500);
  await capture(resultPath, "reopened", { reopened });

  // Pose 5: the grid. One command spreads the open conversation tabs over editor groups; VS Code arranges.
  currentStage = "grid";
  const grid = await journey.arrangeGrid();
  await delay(2_000);
  await capture(resultPath, "grid", grid);

  // Pose 6: the places. The conversation this pass started goes to the bottom panel; another to the secondary
  // side bar; the tabs stay where they are. One conversation, one place, one watch each.
  // Hot sessions only, in two different folders: a conversation the hot ceiling cooled would have to be
  // heated again to be placed, and two conversations of one folder cannot both write there (the Runtime's
  // working-tree contract), so the pass picks what can move right now.
  currentStage = "placing";
  const placing: Record<string, string> = {};
  const hot = journey.sessions().filter((line) => line.hot);
  const forPanel = hot.find((line) => line.sessionId === reopened) ?? hot.find((line) => line.sessionId === session) ?? hot[0] ?? null;
  const panelSession = forPanel?.sessionId ?? null;
  if (panelSession) {
    await journey.placeConversation(panelSession, "panel").catch((error: unknown) => {
      placing.panel = error instanceof Error ? error.message : String(error);
    });
  }
  const sideSession = hot.find((line) => line.sessionId !== panelSession && line.workspace !== forPanel?.workspace)?.sessionId ?? null;
  if (sideSession) {
    await journey.placeConversation(sideSession, "sideBar").catch((error: unknown) => {
      placing.sideBar = error instanceof Error ? error.message : String(error);
    });
  }
  await delay(2_500);
  await capture(resultPath, "places", { panel: panelSession, sideBar: sideSession, failures: placing });

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
  const deletion = await deletionProof(journey, providerIdForDeletion(journey.providers()), resultPath);

  // Leave nothing running: every session this window opened closes, in the isolated Runtime.
  currentStage = "closing";
  for (const line of journey.sessions()) {
    await journey.close(line.sessionId, true).catch(() => undefined);
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
      grid,
      places: { panel: panelSession, sideBar: sideSession, failures: placing },
      fanOut,
      rowTool,
      deletion,
    }),
    "utf8",
  );
}

/// The provider the deletion proof uses: Codex when it is installed and usable (its `thread/delete` is the
/// surface measured), otherwise none.
function providerIdForDeletion(providers: readonly ProviderLine[]): string | null {
  return providers.some((provider) => provider.providerId === "codex" && provider.installation.state === "usable")
    ? "codex"
    : null;
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
    const native = journey.sessions().find((line) => line.sessionId === session)?.nativeSessionId ?? null;
    if (!native) throw new Error("the throwaway conversation announced no native identity");
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
    await capture(resultPath, "diff", { session, approvalsAnswered, approvalsUnanswerable });
    await journey.openLatestDiff();
    await delay(2_500);
    await capture(resultPath, "diffEditor", { session });
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
