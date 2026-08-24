import { access, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import {
  activeConversationEditor,
  allTabs,
  isConversationEditor,
} from "./conversationEditor.test";
import { extensionUnderTest } from "./extensionUnderTest.test";

type ExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
};

let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await verifyInstalledPackage(resultPath);
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

async function verifyInstalledPackage(resultPath: string): Promise<void> {
  const expectedVersion = requiredEnvironment("RUNTROL_TEST_EXTENSION_VERSION");
  const expectedTarget = requiredEnvironment("RUNTROL_TEST_EXTENSION_TARGET");
  const installedRoot = path.resolve(requiredEnvironment("RUNTROL_TEST_INSTALLED_ROOT"));
  const configuredCore = vscode.workspace.getConfiguration("runtrol").get<string>("corePath", "");
  if (configuredCore !== "") {
    throw new Error(`the clean profile unexpectedly configured Core ${configuredCore}`);
  }

  currentStage = "locating-installed-extension";
  const extension = extensionUnderTest<ExtensionApi>();
  if (extension.packageJSON.version !== expectedVersion) {
    throw new Error(`installed version ${String(extension.packageJSON.version)} is not ${expectedVersion}`);
  }
  const installedPath = path.resolve(extension.extensionPath);
  if (installedPath !== installedRoot && !installedPath.startsWith(`${installedRoot}${path.sep}`)) {
    throw new Error(`the extension loaded outside the isolated installation root: ${installedPath}`);
  }

  const executableName = process.platform === "win32" ? "runtrol.exe" : "runtrol";
  const bundledCore = path.join(installedPath, "resources", "core", executableName);
  await access(bundledCore);

  currentStage = "activating-installed-extension";
  const api = await within(extension.activate() as Promise<ExtensionApi>, 10_000, "installed extension activation");
  await within(api.ready, 15_000, "bundled Core discovery");
  currentStage = "refreshing-through-bundled-core";
  await within(api.refresh(), 5_000, "installed extension refresh");
  await within(
    vscode.commands.executeCommand("workbench.view.extension.runtrol"),
    5_000,
    "opening the installed Runtrol view",
  );

  currentStage = "opening-new-conversation";
  const tabsBeforeCommand = new Set(allTabs());
  await within(
    vscode.commands.executeCommand("runtrol.startSession"),
    10_000,
    "opening a new conversation through the public command",
  );
  const newConversationTabs = allTabs().filter(
    (tab) => !tabsBeforeCommand.has(tab) && isConversationEditor(tab),
  );
  if (newConversationTabs.length !== 1) {
    throw new Error(
      `the public new-conversation command opened ${newConversationTabs.length} new Runtrol conversation tabs`,
    );
  }
  const draft = newConversationTabs[0];
  if (activeConversationEditor() !== draft) {
    throw new Error("the new Runtrol conversation draft is not the active editor tab");
  }
  if (draft.label !== "New chat") {
    throw new Error(`the installed new-conversation tab is titled ${draft.label}`);
  }
  const eyeDelay = packageEyeDelay();
  if (eyeDelay > 0) {
    currentStage = "reviewing-new-conversation";
    await delay(eyeDelay);
  }

  currentStage = "closing-new-conversation";
  const accepted = await within(
    vscode.window.tabGroups.close(draft),
    5_000,
    "closing the installed new-conversation draft",
  );
  const draftClosed = accepted && !allTabs().includes(draft);
  if (!draftClosed) {
    throw new Error("the exact installed new-conversation draft remained open after close");
  }

  await writeFile(
    resultPath,
    JSON.stringify({
      vscode: vscode.version,
      extensionVersion: expectedVersion,
      target: expectedTarget,
      extensionPath: installedPath,
      bundledCore,
      configuredCore,
      draftOpened: true,
      draftTitle: draft.label,
      draftClosed,
    }),
    "utf8",
  );
}

function packageEyeDelay(): number {
  const raw = process.env.RUNTROL_TEST_PACKAGE_EYE_DELAY_MS;
  if (raw === undefined) return 0;
  const parsed = Number(raw);
  if (!Number.isSafeInteger(parsed) || parsed < 0 || parsed > 120_000) {
    throw new Error("RUNTROL_TEST_PACKAGE_EYE_DELAY_MS must be an integer from 0 to 120000");
  }
  return parsed;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
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

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}
