import { writeFile } from "node:fs/promises";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";

type ExtensionApi = { readonly ready: Promise<void> };

/// Photograph the empty non-tab chat places in a real Extension Host. No provider session is opened or changed.
export async function run(): Promise<void> {
  const resultPath = process.env.RUNTROL_VSCODE_RESULT;
  if (!resultPath) throw new Error("RUNTROL_VSCODE_RESULT is required");
  try {
    const extension = extensionUnderTest<ExtensionApi>();
    await vscode.commands.executeCommand("workbench.view.extension.runtrol");
    while (!extension.isActive) await delay(25);
    await extension.exports.ready;

    await vscode.commands.executeCommand("runtrol.conversationPanel.focus");
    await delay(1_200);
    await capture(resultPath, "chatPanel");

    await vscode.commands.executeCommand("runtrol.conversationSide.focus");
    await delay(1_200);
    await capture(resultPath, "chatSide");

    await writeFile(resultPath, JSON.stringify({ stage: "complete" }), "utf8");
  } catch (error) {
    await writeFile(resultPath, JSON.stringify({
      stage: "failed",
      failure: error instanceof Error ? error.message : String(error),
    }), "utf8");
    throw error;
  }
}

async function capture(resultPath: string, pose: string): Promise<void> {
  await writeFile(resultPath, JSON.stringify({ stage: `capture:${pose}` }), "utf8");
  const confirmation = `${resultPath}.captured.${pose}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      await import("node:fs/promises").then((files) => files.readFile(confirmation, "utf8"));
      return;
    } catch {
      // The photographer has not confirmed this pose yet.
    }
    if (Date.now() > deadline) throw new Error(`the harness never confirmed the ${pose} capture`);
    await delay(250);
  }
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
