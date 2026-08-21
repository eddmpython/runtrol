import { writeFile } from "node:fs/promises";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

/// A short probe for the places: reopen one stored conversation, put it in the bottom panel, photograph, then
/// the secondary side bar, photograph. Everything real (installed CLI, stored conversation), the window and
/// Runtime isolated. Used to read a place failure without the whole eye pass.
type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  openStoredWithTitle(providerId: string): Promise<string | null>;
  placeConversation(session: string, place: "tab" | "panel" | "sideBar"): Promise<void>;
  nativeChatCount(): number;
  close(session: string, now?: boolean): Promise<void>;
};

type ExtensionApi = { readonly ready: Promise<void>; readonly journey?: JourneyApi };

export async function run(): Promise<void> {
  const resultPath = process.env.RUNTROL_VSCODE_RESULT;
  if (!resultPath) throw new Error("RUNTROL_VSCODE_RESULT is required");
  const providerId = process.env.RUNTROL_EYE_PROVIDER || "claude";
  const report: Record<string, unknown> = {};
  try {
    const extension = extensionUnderTest<ExtensionApi>();
    await vscode.commands.executeCommand("workbench.view.extension.runtrol");
    while (!extension.isActive) await delay(25);
    await extension.exports.ready;
    const journey = extension.exports.journey;
    if (!journey) throw new Error("no journey api");
    const deadline = Date.now() + 90_000;
    while (!journey.providers().some((p) => p.providerId === providerId && p.installation.state === "usable")) {
      if (Date.now() > deadline) throw new Error("provider never verified");
      await delay(100);
    }
    const listed = Date.now() + 90_000;
    while (journey.nativeChatCount() === 0 && Date.now() < listed) await delay(100);
    await delay(4_000);
    const session = await journey.openStoredWithTitle(providerId);
    report.session = session;
    if (!session) throw new Error("no stored conversation to place");
    await delay(1_500);
    try {
      await journey.placeConversation(session, "panel");
      report.panel = "ok";
    } catch (error) {
      report.panel = error instanceof Error ? error.message : String(error);
    }
    await delay(1_500);
    await capture(resultPath, "placePanel", { ...report });
    try {
      await journey.placeConversation(session, "sideBar");
      report.sideBar = "ok";
    } catch (error) {
      report.sideBar = error instanceof Error ? error.message : String(error);
    }
    await delay(1_500);
    await capture(resultPath, "placeSide", { ...report });
    for (const line of journey.sessions()) await journey.close(line.sessionId, true).catch(() => undefined);
    await writeFile(resultPath, JSON.stringify({ stage: "complete", ...report }), "utf8");
  } catch (error) {
    await writeFile(resultPath, JSON.stringify({
      stage: "failed",
      failure: error instanceof Error ? error.message : String(error),
      ...report,
    }), "utf8");
    throw error;
  }
}

async function capture(resultPath: string, pose: string, facts: Record<string, unknown>): Promise<void> {
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}`, ...facts }), "utf8");
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await import("node:fs/promises").then((fs) => fs.readFile(confirmation, "utf8"));
      return;
    } catch {
      // not yet
    }
    if (Date.now() > deadline) throw new Error(`the harness never confirmed the ${pose} capture`);
    await delay(250);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
