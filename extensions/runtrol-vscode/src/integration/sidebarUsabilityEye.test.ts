import { writeFile } from "node:fs/promises";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

type SidebarJourney = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  refreshChats(): Promise<void>;
  nativeChatCount(): number;
  conversationTitles(): readonly string[];
  switchStoredPair(providerId: string): Promise<{
    first: string;
    second: string;
    workspace: string;
    firstLifecycle: SessionLine["lifecycle"];
    secondLifecycle: SessionLine["lifecycle"];
  } | null>;
  close(session: string, now?: boolean): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: SidebarJourney;
};

let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await eyePass(resultPath);
  } catch (error) {
    await writeFile(resultPath, JSON.stringify({
      stage: currentStage,
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    }), "utf8");
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

  currentStage = "waiting-for-sidebar";
  await waitFor(
    () => journey.providers().some((provider) => (
      provider.providerId === providerId && provider.installation.state === "usable"
    )),
    90_000,
    `the installed provider ${providerId}`,
  );
  await within(journey.refreshChats(), 90_000, "refreshing stored conversations");
  await waitFor(() => journey.nativeChatCount() >= 2, 90_000, "two stored conversations");
  await delay(4_000);
  await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar").then(undefined, () => undefined);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  const titles = journey.conversationTitles();
  const untitled = titles.filter((title) => /^Untitled(?:\s|$|[·:])/iu.test(title));
  if (untitled.length > 0) {
    throw new Error(`the sidebar still exposes Untitled rows: ${untitled.slice(0, 5).join(" | ")}`);
  }
  const internalHandles = titles.filter((title) => /^Chat [A-Z0-9]{1,8}$/u.test(title));
  if (internalHandles.length > 0) {
    throw new Error(`the sidebar exposes internal chat handles: ${internalHandles.slice(0, 5).join(" | ")}`);
  }
  await capture(resultPath, "sidebar", {
    conversations: titles.length,
    untitled: untitled.length,
    internalHandles: internalHandles.length,
  });

  currentStage = "switching-saved-chats";
  const switched = await within(journey.switchStoredPair(providerId), 120_000, "switching two saved chats");
  if (!switched) throw new Error(`no two titled ${providerId} chats in one workspace could be reopened`);
  if (switched.firstLifecycle !== "cold" || switched.secondLifecycle !== "hotIdle") {
    throw new Error(
      `saved-chat switch ended ${switched.firstLifecycle} -> ${switched.secondLifecycle}, expected cold -> hotIdle`,
    );
  }
  await delay(1_500);
  await vscode.commands.executeCommand("notifications.clearAll").then(undefined, () => undefined);
  await capture(resultPath, "switched", switched);

  currentStage = "closing";
  for (const session of journey.sessions()) {
    await journey.close(session.sessionId, true).catch(() => undefined);
  }
  await writeFile(resultPath, JSON.stringify({ stage: "complete", vscode: vscode.version, switched }), "utf8");
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  currentStage = `capture:${pose}`;
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}`, ...facts }), "utf8");
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
