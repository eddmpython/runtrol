import { writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";

type ExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
};

const continuityTimeoutMs = 15_000;
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
  await within(api.ready, 15_000, `${phase} Core discovery`);
  await within(api.refresh(), 5_000, `${phase} refresh`);

  if (expectedSession) {
    currentStage = `${phase}-selection`;
    await within(
      vscode.commands.executeCommand("workbench.view.extension.runtrol"),
      5_000,
      `${phase} view opening`,
    );
    await within(
      vscode.commands.executeCommand("runtrol.openConversation"),
      5_000,
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
  const tab = await within(waitForConversationEditor(), 15_000, "registering the conversation editor tab");
  if (!tab || !tab.label.startsWith("Runtrol:")) {
    throw new Error("the selected conversation is not an identifiable editor Webview tab");
  }
}

async function waitForConversationEditor(): Promise<vscode.Tab | null> {
  for (;;) {
    const tab = vscode.window.tabGroups.all
      .flatMap((group) => group.tabs)
      .find((candidate) => candidate.isActive && candidate.label.startsWith("Runtrol:"));
    if (tab) {
      return tab;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}
