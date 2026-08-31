import { writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import {
  allTabs,
  conversationTabDiagnostics,
  isConversationEditor,
} from "./conversationEditor.test";
import { extensionUnderTest } from "./extensionUnderTest.test";

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
};

const continuityTimeoutMs = 15_000;
const initializationDeadlineMs = 60_000;
let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await verifyPhase(resultPath);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        stage: currentStage,
      }),
      "utf8",
    );
    throw error;
  }
}

async function verifyPhase(resultPath: string): Promise<void> {
  const expectedVersion = requiredEnvironment("RUNTROL_TEST_EXTENSION_VERSION");
  const expectedWorkspace = normalize(requiredEnvironment("RUNTROL_TEST_WORKSPACE"));
  const expectedSession = process.env.RUNTROL_TEST_SESSION;
  const phase = requiredEnvironment("RUNTROL_VSCODE_PHASE");
  const extension = extensionUnderTest<ExtensionApi>();
  if (extension.packageJSON.version !== expectedVersion) {
    throw new Error(`installed version ${String(extension.packageJSON.version)} is not ${expectedVersion}`);
  }
  const openWorkspace = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!openWorkspace || normalize(openWorkspace) !== expectedWorkspace) {
    throw new Error(`the Extension Host opened ${String(openWorkspace)}, expected ${expectedWorkspace}`);
  }

  currentStage = `${phase}-activation`;
  const api = await within(extension.activate() as Promise<ExtensionApi>, 10_000, `${phase} activation`);
  currentStage = `${phase}-initialization`;
  try {
    // A phase starts a fresh installed Extension Host. The bootstrap phase can also start the first daemon for a
    // fresh home, so this is a hang boundary rather than a performance budget. The dedicated host performance
    // gate owns activation latency; upgrade continuity must not fail first-daemon assembly on a loaded CI disk.
    await within(api.ready, initializationDeadlineMs, `${phase} Core discovery`);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(
      `${phase} initialization failed at ${api.initializationStage ?? "unknown"}: ${detail}`,
      { cause: error },
    );
  }
  currentStage = `${phase}-refresh`;
  await within(api.refresh(), continuityTimeoutMs, `${phase} refresh`);

  if (expectedSession) {
    currentStage = `${phase}-selection`;
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      continuityTimeoutMs,
      `${phase} view opening`,
    );
    await within(
      vscode.commands.executeCommand("runtrol.openConversation"),
      continuityTimeoutMs,
      `${phase} conversation focus`,
    );
    await requireConversationEditor();
    if (!api.verifyRestoredSession) {
      throw new Error("the bounded restored-session verifier is unavailable");
    }
    await within(
      api.verifyRestoredSession(expectedSession),
      continuityTimeoutMs,
      `${phase} selected-session restore`,
    );
  }

  await writeFile(
    resultPath,
    JSON.stringify({
      phase,
      vscode: vscode.version,
      extensionVersion: expectedVersion,
      extensionPath: extension.extensionPath,
      workspace: openWorkspace,
      session: expectedSession ?? null,
    }),
    "utf8",
  );
}

function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    Promise.resolve(work),
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) {
      clearTimeout(timer);
    }
  });
}

function normalize(value: string): string {
  const resolved = path.resolve(value);
  return process.platform === "win32" ? resolved.toLocaleLowerCase("en-US") : resolved;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
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
  if (!tab || !tab.label.trim() || tab.label.startsWith("Runtrol")) {
    throw new Error("the selected conversation is not an identifiable editor Webview tab");
  }
}

async function waitForConversationEditor(): Promise<vscode.Tab | null> {
  for (;;) {
    const tab = allTabs().find(isConversationEditor);
    if (tab) {
      return tab;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}
