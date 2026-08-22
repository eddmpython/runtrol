import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";

import * as vscode from "vscode";

import {
  allTabs,
  conversationTabDiagnostics,
  isConversationEditor,
} from "./conversationEditor.test";
import { extensionUnderTest } from "./extensionUnderTest.test";

let currentStage = "starting";
const EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS = 15_000;
const SESSION_MANAGEMENT_HANG_TIMEOUT_MS = 30_000;

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  measureWebview?(framesPerSecond?: number, durationMs?: number): Promise<{
    baselineFrameP95Ms: number;
    frameP95Ms: number;
    frameOverrunP95Ms: number;
    inputP95Ms: number;
    scrollP95Ms: number;
    maxPendingFrames: number;
    producedFrames: number;
    droppedFrames: number;
    visibleCharacters: number;
    visibleItems: number;
  }>;
  measureSessionManagement?(sessionIds: readonly string[]): Promise<{
    sessionCount: number;
    hotSessionCount: number;
    coldResumeMs: number;
    sessionSwitchP95Ms: number;
    resumedFrom: string;
    resumedTo: string;
    restoreSession: string;
    restoreWorkspace: string;
  }>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
  hasConversationIn?(folder: string): Promise<boolean>;
  openFirstConversation?(): Promise<void>;
  openCrossProjectConversation?(): Promise<void>;
  waitForConversationIn?(folder: string, deadlineMs: number): Promise<number>;
  seedProject?(folder: string): Promise<void>;
};

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    if (process.env.RUNTROL_VSCODE_PHASE === "restore") {
      const restored = await measureRestore(requiredEnvironment("RUNTROL_VSCODE_RESTORE_SESSION"));
      await writeFile(resultPath, JSON.stringify(restored), "utf8");
    } else if (process.env.RUNTROL_VSCODE_PHASE === "follow") {
      const followed = await measureFollow(requiredEnvironment("RUNTROL_VSCODE_FOLLOW_TARGET"), resultPath);
      await writeFile(resultPath, JSON.stringify(followed), "utf8");
    } else {
      const measured = await measure(resultPath);
      await writeFile(resultPath, JSON.stringify(measured), "utf8");
    }
  } catch (error) {
    let progress: Record<string, unknown> = {};
    try {
      const recorded: unknown = JSON.parse(await readFile(resultPath, "utf8"));
      if (recorded && typeof recorded === "object" && !Array.isArray(recorded)) {
        progress = recorded as Record<string, unknown>;
      }
    } catch {
      // A failure before the first checkpoint has no progress record to preserve.
    }
    await writeFile(
      resultPath,
      JSON.stringify({
        ...progress,
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        stage: currentStage,
      }),
      "utf8",
    );
    throw error;
  }
}

async function measure(resultPath: string): Promise<Record<string, number | string>> {
  const core = requiredEnvironment("RUNTROL_TEST_CORE");
  const configuredCore = vscode.workspace.getConfiguration("runtrol").get<string>("corePath");
  if (configuredCore !== core) {
    throw new Error(`the isolated VS Code profile configured unexpected Core ${String(configuredCore)}`);
  }
  const extension = extensionUnderTest<ExtensionApi>();
  const rssBefore = process.memoryUsage().rss;
  const activationStarted = performance.now();
  const api = await within(extension.activate() as Promise<ExtensionApi>, 5_000, "extension activation");
  await checkpoint(resultPath, "activated");
  try {
    await within(api.ready, EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS, "extension initialization");
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(
      `extension initialization failed at ${api.initializationStage ?? "unknown"}: ${detail}`,
      { cause: error },
    );
  }
  const activationMs = performance.now() - activationStarted;
  await checkpoint(resultPath, "ready");

  await checkpoint(resultPath, "view-opening");
  const viewStarted = performance.now();
  try {
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      5_000,
      "opening the Runtrol view",
    );
  } catch (error) {
    throw new Error(`Runtrol view command failed: ${error instanceof Error ? error.message : String(error)}`, {
      cause: error,
    });
  }
  await within(
    vscode.commands.executeCommand("runtrol.openConversation"),
    5_000,
    "focusing the Runtrol Webview",
  );
  await requireConversationEditor();
  const openViewMs = performance.now() - viewStarted;
  await checkpoint(resultPath, "view-open");
  await hideAndRestoreConversation();

  for (let index = 0; index < 5; index += 1) {
    await within(api.refresh(), 5_000, "refresh warmup");
  }
  await checkpoint(resultPath, "warmed");
  const refreshSamples: number[] = [];
  for (let index = 0; index < 40; index += 1) {
    const started = performance.now();
    await within(api.refresh(), 5_000, "measured refresh");
    refreshSamples.push(performance.now() - started);
  }

  currentStage = "webview-load";
  if (!api.measureWebview) {
    throw new Error("the performance-only Webview measurement API is unavailable");
  }
  const framesPerSecond = numericEnvironment("RUNTROL_VSCODE_PERFORMANCE_RATE", 3_000);
  const durationMs = numericEnvironment("RUNTROL_VSCODE_PERFORMANCE_DURATION", 5_000);
  const webview = await within(
    api.measureWebview(framesPerSecond, durationMs),
    30_000,
    "Webview burst measurement",
  );
  const expectedFrames = Math.ceil(framesPerSecond * durationMs / 1_000);
  if (webview.producedFrames < expectedFrames) {
    throw new Error(`Webview load produced only ${webview.producedFrames} frames`);
  }
  if (webview.droppedFrames !== 0) {
    throw new Error(`Webview transport dropped ${webview.droppedFrames} raw frames`);
  }
  if (webview.visibleItems > 400 || webview.visibleCharacters > 256 * 1024) {
    throw new Error(
      `Webview bounds escaped at ${webview.visibleItems} items and ${webview.visibleCharacters} characters`,
    );
  }

  currentStage = "session-switch";
  if (!api.measureSessionManagement) {
    throw new Error("the performance-only hot-session measurement API is unavailable");
  }
  const switched = await within(
    api.measureSessionManagement(managedSessionIds()),
    SESSION_MANAGEMENT_HANG_TIMEOUT_MS,
    "30-session management and eight hot session switches",
  );

  const result = {
    vscode: vscode.version,
    activationMs,
    openViewMs,
    refreshP95Ms: percentile(refreshSamples, 0.95),
    rssGrowthBytes: Math.max(0, process.memoryUsage().rss - rssBefore),
    webviewFrameP95Ms: webview.frameP95Ms,
    webviewBaselineFrameP95Ms: webview.baselineFrameP95Ms,
    webviewFrameOverrunP95Ms: webview.frameOverrunP95Ms,
    webviewInputP95Ms: webview.inputP95Ms,
    webviewScrollP95Ms: webview.scrollP95Ms,
    webviewPendingFrames: webview.maxPendingFrames,
    webviewDroppedFrames: webview.droppedFrames,
    sessionCount: switched.sessionCount,
    hotSessionCount: switched.hotSessionCount,
    coldResumeMs: switched.coldResumeMs,
    sessionSwitchP95Ms: switched.sessionSwitchP95Ms,
    resumedFrom: switched.resumedFrom,
    resumedTo: switched.resumedTo,
    restoreSession: switched.restoreSession,
    restoreWorkspace: switched.restoreWorkspace,
  };
  await writeFile(resultPath, JSON.stringify(result), "utf8");
  return result;
}

async function measureRestore(expected: string): Promise<{
  reloadRestoreMs: number;
  reloadActivationMs: number;
  reloadReadyMs: number;
  reloadViewMs: number;
  reloadSelectionMs: number;
}> {
  currentStage = "reload-activation";
  const extension = extensionUnderTest<ExtensionApi>();
  const started = performance.now();
  const api = await within(extension.activate() as Promise<ExtensionApi>, 5_000, "reload activation");
  const activatedAt = performance.now();
  let readyAt = activatedAt;
  let viewAt = activatedAt;
  const ready = within(api.ready, EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS, "reload initialization").then(() => {
    readyAt = performance.now();
  });
  const view = (async () => {
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      5_000,
      "opening the Runtrol view after reload",
    );
    await within(
      vscode.commands.executeCommand("runtrol.openConversation"),
      5_000,
      "focusing the Runtrol Webview after reload",
    );
    await requireConversationEditor();
    viewAt = performance.now();
  })();
  await Promise.all([ready, view]);
  const readyAndViewAt = Math.max(readyAt, viewAt);
  if (!api.verifyRestoredSession) {
    throw new Error("the performance-only restored-session verifier is unavailable");
  }
  currentStage = "reload-selection";
  await within(api.verifyRestoredSession(expected), 5_000, "restoring the selected hot session");
  const selectedAt = performance.now();
  const reloadRestoreMs = selectedAt - started;
  const reloadActivationMs = activatedAt - started;
  const reloadReadyMs = readyAt - activatedAt;
  const reloadViewMs = viewAt - activatedAt;
  const reloadSelectionMs = selectedAt - readyAndViewAt;
  return {
    reloadRestoreMs,
    reloadActivationMs,
    reloadReadyMs,
    reloadViewMs,
    reloadSelectionMs,
  };
}

/// The live proof that a folder opened later still gets its conversations.
///
/// The window starts from a saved `.code-workspace` naming one folder, so adding a second folder keeps the same
/// extension host alive (a plain-folder window would restart it, which would prove a restart and not a follow).
/// The second folder was never granted by any earlier phase, so its conversation can only arrive through the
/// live chain: folder event, grant widened, discovery listed, index broadcast, row rendered.
async function measureFollow(target: string, resultPath: string): Promise<Record<string, number | string>> {
  currentStage = "follow-activation";
  const extension = extensionUnderTest<ExtensionApi>();
  const api = await within(extension.activate() as Promise<ExtensionApi>, 5_000, "follow activation");
  await within(api.ready, EXTENSION_INITIALIZATION_HANG_TIMEOUT_MS, "follow initialization");
  if (!api.hasConversationIn || !api.waitForConversationIn) {
    throw new Error("the performance-only follow probes are unavailable");
  }
  const openFolders = vscode.workspace.workspaceFolders ?? [];
  const first = openFolders[0]?.uri.fsPath;
  if (!first || openFolders.length !== 1) {
    throw new Error(`the follow phase expects exactly one starting folder, found ${openFolders.length}`);
  }
  currentStage = "follow-first-folder";
  // The starting folder's stored conversation arriving proves discovery works at all in this window, so a
  // later failure on the added folder indicts the follow chain and nothing else.
  await api.waitForConversationIn(first, 30_000);
  if (await api.hasConversationIn(target)) {
    throw new Error(`${target} was visible before it was ever opened, so this phase can prove nothing`);
  }
  currentStage = "follow-add-folder";
  const added = vscode.workspace.updateWorkspaceFolders(openFolders.length, 0, {
    uri: vscode.Uri.file(target),
  });
  if (!added) {
    throw new Error("VS Code refused to add the second workspace folder");
  }
  const followArrivalMs = await api.waitForConversationIn(target, 30_000);
  currentStage = "cross-project-open";
  // The operator's exact pain, held as a gate (memory/uxContract.md): a conversation whose folder this
  // window has NOT opened must select and open as a conversation tab right here, without moving the window.
  if (!api.openCrossProjectConversation) {
    throw new Error("the performance-only cross-project opener is unavailable");
  }
  await within(
    api.openCrossProjectConversation(),
    15_000,
    "opening a conversation from an unopened folder",
  );
  await requireConversationEditor();
  await eyePass(api, first, resultPath);
  return { vscode: vscode.version, followArrivalMs };
}

/// The opt-in screenshot moment: stand the panel up the way the operator described it and hold still while the
/// harness photographs the window. The picture then has one created project holding the first folder's
/// conversation, the second folder's conversation loose beneath the headings, and the usage strip at the bottom.
///
/// Does nothing unless RUNTROL_VSCODE_CAPTURE names an output file, so the gate's timed runs never pay for it.
async function eyePass(api: ExtensionApi, projectFolder: string, resultPath: string): Promise<void> {
  if (!process.env.RUNTROL_VSCODE_CAPTURE) return;
  currentStage = "eye-pass";
  if (!api.seedProject) {
    throw new Error("the performance-only project seeder is unavailable");
  }
  await api.seedProject(projectFolder);
  // A project this window is NOT open on, so the photograph shows the move button that only draws on
  // non-current headings (the contract: moving the window is the one explicit act).
  const elsewhere = path.join(os.tmpdir(), "runtrol-eyepass-elsewhere");
  await mkdir(elsewhere, { recursive: true });
  await api.seedProject(elsewhere);
  if (!api.openFirstConversation) {
    throw new Error("the performance-only conversation opener is unavailable");
  }
  // A conversation on screen, so the photograph includes the header chips fed by real announcements.
  await within(
    api.openFirstConversation(),
    20_000,
    "opening a conversation for the eye pass",
  );
  await within(
    vscode.commands.executeCommand("workbench.view.extension.runtrol"),
    5_000,
    "opening the Runtrol view for the eye pass",
  );
  await within(
    vscode.commands.executeCommand("runtrol.usage.focus"),
    5_000,
    "revealing the usage strip for the eye pass",
  );
  // One breath for the tree to paint the new heading before the photograph.
  await new Promise((resolve) => setTimeout(resolve, 1_500));
  await checkpoint(resultPath, "capture-ready");
  const captured = `${resultPath}.captured`;
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      await readFile(captured, "utf8");
      return;
    } catch {
      // Not written yet. The harness is still photographing; keep waiting until the deadline says otherwise.
    }
    if (Date.now() > deadline) {
      throw new Error("the harness never confirmed the eye-pass capture");
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}

async function checkpoint(resultPath: string, stage: string): Promise<void> {
  currentStage = stage;
  await writeFile(resultPath, JSON.stringify({ stage }), "utf8");
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
    if (timer) {
      clearTimeout(timer);
    }
  }
}

function percentile(values: readonly number[], at: number): number {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * at) - 1] ?? Number.POSITIVE_INFINITY;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function numericEnvironment(name: string, fallback: number): number {
  const raw = process.env[name];
  const value = raw ? Number(raw) : fallback;
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${name} must be a positive number`);
  }
  return value;
}

function managedSessionIds(): string[] {
  const raw = requiredEnvironment("RUNTROL_VSCODE_MANAGED_SESSIONS");
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new Error("RUNTROL_VSCODE_MANAGED_SESSIONS is not JSON");
  }
  if (!Array.isArray(value) || value.length !== 30 || !value.every((item) => typeof item === "string")) {
    throw new Error("RUNTROL_VSCODE_MANAGED_SESSIONS must contain 30 session identifiers");
  }
  return value;
}

async function requireConversationEditor(): Promise<void> {
  let tab: vscode.Tab | null;
  try {
    tab = await within(waitForConversationEditor(), 15_000, "registering the conversation editor tab");
  } catch (error) {
    throw new Error(
      `the conversation editor tab was not registered; current tabs: ${conversationTabDiagnostics()}`,
      { cause: error },
    );
  }
  if (!tab) {
    throw new Error("the Runtrol conversation did not open as an editor Webview tab");
  }
  // Activation settles a tick after registration while VS Code moves tab-group state; the contract is
  // that the tab ENDS active, so the check waits the same bounded way registration did instead of
  // photographing one transitional frame.
  const activeBy = Date.now() + 5_000;
  while (!tab.isActive) {
    if (Date.now() > activeBy) {
      throw new Error("the Runtrol conversation editor tab is not active");
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
    const refreshed = allTabs();
    tab = refreshed.find((candidate) => isConversationEditor(candidate) && candidate.isActive)
      ?? refreshed.find(isConversationEditor)
      ?? tab;
  }
  if (!tab.label.startsWith("Runtrol")) {
    throw new Error(`the conversation editor has an unreadable label: ${tab.label}`);
  }
}

async function waitForConversationEditor(): Promise<vscode.Tab | null> {
  for (;;) {
    // With one tab per conversation there can be several registered at once (restored tabs included);
    // the assertion is about the tab the reader is IN, so the active one wins the search.
    const tabs = allTabs();
    const tab = tabs.find((candidate) => isConversationEditor(candidate) && candidate.isActive)
      ?? tabs.find(isConversationEditor);
    if (tab) {
      return tab;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}

async function hideAndRestoreConversation(): Promise<void> {
  const temporary = await vscode.workspace.openTextDocument({ content: "Runtrol editor lifecycle check\n" });
  await vscode.window.showTextDocument(temporary, { preview: true });
  const activeTab = vscode.window.tabGroups.activeTabGroup.activeTab;
  if (activeTab && isConversationEditor(activeTab)) {
    throw new Error("a text editor did not hide the Runtrol conversation tab");
  }
  await within(
    vscode.commands.executeCommand("runtrol.openConversation"),
    5_000,
    "restoring the hidden Runtrol conversation",
  );
  await requireConversationEditor();
}
